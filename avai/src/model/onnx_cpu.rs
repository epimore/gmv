use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use base::{
    serde::Serialize,
    tokio::sync::{Mutex as AsyncMutex, Notify, oneshot},
    tokio_util::sync::CancellationToken,
};
use image::imageops::FilterType;
use ort::{
    session::{RunOptions, Session},
    value::{Tensor, TensorElementType, ValueType},
};

use super::{
    ExecutionContract, ExecutionLimits, InferenceResult, InstalledModel, ModelError, ModelIdentity,
    ModelInstance, ModelResult, RuntimeCallContext, RuntimeDescriptor, RuntimeInput,
    RuntimeProvider, SelfTestCase, TensorContract,
    package::{
        actual_model, checked_element_count, load_installed_execution_contract,
        validate_execution_contract,
    },
    runtime::{RuntimeFuture, compare_json_numeric},
};

pub const ONNX_CPU_RUNTIME: &str = "onnx-cpu";
pub const ONNX_RUNTIME_VERSION: &str = "1.28.0";
const ONNX_CONTRACT_VERSION: u32 = 1;
const ONNX_RUNTIME_GIT_COMMIT: &str = "da9b5e364c";
const MAX_NATIVE_WORKERS: usize = 64;

#[derive(Debug, Clone, Copy)]
pub struct OnnxCpuConfig {
    pub worker_count: usize,
    pub queue_capacity: usize,
    pub intra_threads: usize,
    pub inter_threads: usize,
    pub max_result_bytes: usize,
    pub shutdown_timeout: Duration,
    pub execution_limits: ExecutionLimits,
}

impl Default for OnnxCpuConfig {
    fn default() -> Self {
        Self {
            worker_count: 1,
            queue_capacity: 8,
            intra_threads: 1,
            inter_threads: 1,
            max_result_bytes: 1024 * 1024,
            shutdown_timeout: Duration::from_secs(5),
            execution_limits: ExecutionLimits::default(),
        }
    }
}

#[derive(Clone)]
pub struct OnnxCpuProvider {
    executor: NativeExecutor,
    config: OnnxCpuConfig,
    instances_created: Arc<AtomicUsize>,
    instances_dropped: Arc<AtomicUsize>,
}

impl OnnxCpuProvider {
    pub fn initialize_from_release(config: OnnxCpuConfig) -> ModelResult<Self> {
        let executable = std::env::current_exe()
            .map_err(|error| ModelError::io("resolve AVAI executable", error))?;
        let bin = executable.parent().ok_or_else(|| {
            ModelError::new(
                "model_runtime_unavailable",
                "AVAI executable has no release bin directory",
            )
        })?;
        let release = bin.parent().ok_or_else(|| {
            ModelError::new(
                "model_runtime_unavailable",
                "AVAI executable is not inside a trusted release layout",
            )
        })?;
        Self::initialize(
            release.join(format!(
                "lib/onnxruntime/{ONNX_RUNTIME_VERSION}/libonnxruntime.so.{ONNX_RUNTIME_VERSION}"
            )),
            config,
        )
    }

    #[cfg(any(test, feature = "native-onnx-tests"))]
    pub fn initialize_for_test(
        trusted_library: impl AsRef<Path>,
        config: OnnxCpuConfig,
    ) -> ModelResult<Self> {
        Self::initialize(trusted_library.as_ref().to_path_buf(), config)
    }

    fn initialize(trusted_library: PathBuf, config: OnnxCpuConfig) -> ModelResult<Self> {
        validate_config(config)?;
        let metadata = std::fs::symlink_metadata(&trusted_library).map_err(|_| {
            ModelError::new(
                "model_runtime_unavailable",
                "trusted ONNX Runtime library is unavailable",
            )
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ModelError::new(
                "model_runtime_unavailable",
                "trusted ONNX Runtime library must be a regular non-symlink file",
            ));
        }
        ort::init_from(&trusted_library)
            .map_err(|error| runtime_error("load ONNX Runtime library", error))?
            .with_name("avai-onnx-cpu")
            .with_telemetry(false)
            .commit();
        let build_info = ort::info();
        if !build_info.contains(&format!("git-commit-id={ONNX_RUNTIME_GIT_COMMIT}")) {
            return Err(ModelError::new(
                "model_runtime_version_mismatch",
                "loaded ONNX Runtime is not the approved 1.28.0 build",
            ));
        }
        Ok(Self {
            executor: NativeExecutor::new(config.worker_count, config.queue_capacity)?,
            config,
            instances_created: Arc::new(AtomicUsize::new(0)),
            instances_dropped: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub async fn close_and_wait(&self) -> ModelResult<()> {
        self.executor
            .close_and_wait(Instant::now() + self.config.shutdown_timeout)
            .await
    }

    #[cfg(any(test, feature = "native-onnx-tests"))]
    pub fn lifetime_snapshot(&self) -> NativeRuntimeSnapshot {
        NativeRuntimeSnapshot {
            accepting: self.executor.inner.accepting.load(Ordering::Acquire),
            admitted_jobs: self.executor.inner.admitted_jobs.load(Ordering::Acquire),
            completed_jobs: self.executor.inner.completed_jobs.load(Ordering::Acquire),
            active_jobs: self.executor.inner.active_jobs.load(Ordering::Acquire),
            workers_remaining: self
                .executor
                .inner
                .workers_remaining
                .load(Ordering::Acquire),
            instances_created: self.instances_created.load(Ordering::Acquire),
            instances_dropped: self.instances_dropped.load(Ordering::Acquire),
        }
    }
}

#[cfg(any(test, feature = "native-onnx-tests"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeRuntimeSnapshot {
    pub accepting: bool,
    pub admitted_jobs: usize,
    pub completed_jobs: usize,
    pub active_jobs: usize,
    pub workers_remaining: usize,
    pub instances_created: usize,
    pub instances_dropped: usize,
}

fn validate_config(config: OnnxCpuConfig) -> ModelResult<()> {
    if config.worker_count == 0
        || config.worker_count > MAX_NATIVE_WORKERS
        || config.queue_capacity == 0
        || config.intra_threads == 0
        || config.inter_threads == 0
        || config.max_result_bytes == 0
        || config.shutdown_timeout.is_zero()
        || config.execution_limits.max_input_elements == 0
        || config.execution_limits.max_input_bytes == 0
        || config.execution_limits.max_output_tensors == 0
        || config.execution_limits.max_output_elements == 0
        || config.execution_limits.max_output_bytes == 0
    {
        return Err(ModelError::new(
            "invalid_model_runtime_config",
            "ONNX CPU executor and result bounds must be positive and capped",
        ));
    }
    Ok(())
}

fn validate_onnx_cpu_selector(selector: &super::RuntimeVariant) -> ModelResult<()> {
    if selector.runtime_contract_version != ONNX_CONTRACT_VERSION {
        return Err(ModelError::new(
            "model_runtime_contract_unsupported",
            "onnx-cpu supports only runtime contract version 1",
        ));
    }
    if selector.accelerator != "cpu" {
        return Err(ModelError::new(
            "model_accelerator_unavailable",
            "onnx-cpu requires the exact cpu accelerator selector",
        ));
    }
    Ok(())
}

impl RuntimeProvider for OnnxCpuProvider {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            runtime: ONNX_CPU_RUNTIME.to_string(),
            version: ONNX_RUNTIME_VERSION.to_string(),
        }
    }

    fn validate_selector(&self, selector: &super::RuntimeVariant) -> ModelResult<()> {
        validate_onnx_cpu_selector(selector)
    }

    fn preload<'a>(
        &'a self,
        model: &'a InstalledModel,
        context: RuntimeCallContext,
    ) -> RuntimeFuture<'a, Arc<dyn ModelInstance>> {
        Box::pin(async move {
            context.ensure_active()?;
            let installed = load_installed_execution_contract(model)?;
            if installed.selected_variant.runtime != ONNX_CPU_RUNTIME
                || installed.selected_variant.runtime_contract_version != ONNX_CONTRACT_VERSION
                || installed.selected_variant.accelerator != "cpu"
            {
                return Err(ModelError::new(
                    "model_runtime_contract_mismatch",
                    "selected variant is not onnx-cpu contract v1 on CPU",
                ));
            }
            let execution = installed.execution.ok_or_else(|| {
                ModelError::new(
                    "model_execution_contract_missing",
                    "onnx-cpu requires execution contract v1",
                )
            })?;
            validate_execution_contract(
                &execution,
                &self.config.execution_limits,
                model.resources.memory_mb,
            )?;
            if installed.self_tests.is_empty()
                || installed.self_tests.iter().any(|case| {
                    case.oracle
                        .as_ref()
                        .is_none_or(|oracle| oracle.kind != "json_numeric_v1")
                })
            {
                return Err(ModelError::new(
                    "model_self_test_contract_missing",
                    "onnx-cpu requires at least one json_numeric_v1 self-test",
                ));
            }
            let artifact = model
                .installed_path
                .join(&installed.selected_variant.artifact);
            let session = self
                .executor
                .load_session(
                    context,
                    artifact,
                    self.config.intra_threads,
                    self.config.inter_threads,
                )
                .await?;
            validate_session(&session, &execution)?;
            self.instances_created.fetch_add(1, Ordering::AcqRel);
            Ok(Arc::new(OnnxCpuInstance {
                identity: model.identity.clone(),
                capabilities: model.capabilities.clone(),
                installed_path: model.installed_path.clone(),
                execution,
                self_tests: installed.self_tests,
                session: Arc::new(Mutex::new(session)),
                executor: self.executor.clone(),
                max_result_bytes: self.config.max_result_bytes,
                instances_dropped: self.instances_dropped.clone(),
            }) as Arc<dyn ModelInstance>)
        })
    }
}

struct OnnxCpuInstance {
    identity: ModelIdentity,
    capabilities: Vec<String>,
    installed_path: PathBuf,
    execution: ExecutionContract,
    self_tests: Vec<SelfTestCase>,
    session: Arc<Mutex<Session>>,
    executor: NativeExecutor,
    max_result_bytes: usize,
    instances_dropped: Arc<AtomicUsize>,
}

impl Drop for OnnxCpuInstance {
    fn drop(&mut self) {
        self.instances_dropped.fetch_add(1, Ordering::AcqRel);
    }
}

impl ModelInstance for OnnxCpuInstance {
    fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    fn runtime(&self) -> &str {
        ONNX_CPU_RUNTIME
    }

    fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    fn unload<'a>(&'a self, context: RuntimeCallContext) -> RuntimeFuture<'a, ()> {
        Box::pin(async move { context.ensure_active() })
    }

    fn self_test<'a>(
        &'a self,
        cases: &'a [SelfTestCase],
        context: RuntimeCallContext,
    ) -> RuntimeFuture<'a, ()> {
        Box::pin(async move {
            let requested = base::serde_json::to_vec(cases)
                .map_err(|error| ModelError::io("encode requested model self-tests", error))?;
            let installed = base::serde_json::to_vec(&self.self_tests)
                .map_err(|error| ModelError::io("encode installed model self-tests", error))?;
            if requested != installed {
                return Err(ModelError::new(
                    "model_package_changed",
                    "durable self-tests do not match the immutable manifest",
                ));
            }
            for case in &self.self_tests {
                let input = self.read_self_test_input(case)?;
                let actual = self.run(input, context.clone()).await?;
                let expected = std::fs::read(self.installed_path.join(&case.expected))
                    .map_err(|error| ModelError::io("read model self-test oracle", error))?;
                compare_json_numeric(
                    &actual,
                    &expected,
                    case.oracle.as_ref().ok_or_else(|| {
                        ModelError::new(
                            "model_self_test_contract_missing",
                            "self-test numeric oracle is missing",
                        )
                    })?,
                )?;
            }
            Ok(())
        })
    }

    fn health<'a>(&'a self, context: RuntimeCallContext) -> RuntimeFuture<'a, ()> {
        Box::pin(async move {
            let case = self.self_tests.first().ok_or_else(|| {
                ModelError::new("model_health_failed", "model has no bounded health case")
            })?;
            let actual = self.run(self.read_self_test_input(case)?, context).await?;
            let expected = std::fs::read(self.installed_path.join(&case.expected))
                .map_err(|error| ModelError::io("read model health oracle", error))?;
            compare_json_numeric(
                &actual,
                &expected,
                case.oracle.as_ref().ok_or_else(|| {
                    ModelError::new("model_health_failed", "model health oracle is missing")
                })?,
            )
            .map_err(|_| ModelError::new("model_health_failed", "model health inference failed"))
        })
    }

    fn infer<'a>(
        &'a self,
        input: RuntimeInput,
        context: RuntimeCallContext,
    ) -> RuntimeFuture<'a, InferenceResult> {
        Box::pin(async move {
            let output = self.run(input, context).await?;
            Ok(InferenceResult {
                output,
                actual_model: actual_model(&self.identity, ONNX_CPU_RUNTIME),
            })
        })
    }
}

impl OnnxCpuInstance {
    fn read_self_test_input(&self, case: &SelfTestCase) -> ModelResult<RuntimeInput> {
        let encoded = std::fs::read(self.installed_path.join(&case.input))
            .map_err(|error| ModelError::io("read model self-test input", error))?;
        let image = image::load_from_memory(&encoded)
            .map_err(|error| runtime_error("decode model self-test input", error))?;
        let media_type = media_type_from_path(&case.input)?;
        Ok(RuntimeInput {
            encoded: encoded.into(),
            media_type: media_type.to_string(),
            width: image.width(),
            height: image.height(),
        })
    }

    async fn run(&self, input: RuntimeInput, context: RuntimeCallContext) -> ModelResult<Vec<u8>> {
        context.ensure_active()?;
        let tensor_data = preprocess(&self.execution, &input)?;
        let tensor_shape = self
            .execution
            .input
            .tensor
            .shape
            .iter()
            .map(|value| usize::try_from(*value))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| {
                ModelError::new(
                    "model_input_contract_mismatch",
                    "input tensor shape does not fit this platform",
                )
            })?;
        let tensor = Tensor::from_array((tensor_shape, tensor_data.into_boxed_slice()))
            .map_err(|error| runtime_error("create ONNX input tensor", error))?;
        let input_name = self.execution.input.tensor.name.clone();
        let output_contract = self.execution.outputs.clone();
        let session = self.session.clone();
        let run_options = Arc::new(
            RunOptions::new().map_err(|error| runtime_error("create ONNX run options", error))?,
        );
        let terminate = run_options.clone();
        let max_result_bytes = self.max_result_bytes;
        self.executor
            .execute(
                context,
                move || terminate.terminate(),
                move || {
                    let mut session = session.lock().map_err(|_| {
                        ModelError::new("model_runtime_failed", "ONNX session lock is poisoned")
                    })?;
                    let outputs = session
                        .run_with_options(vec![(input_name.as_str(), tensor)], &run_options)
                        .map_err(|error| runtime_error("run ONNX inference", error))?;
                    encode_outputs(&outputs, &output_contract, max_result_bytes)
                },
            )
            .await
    }
}

fn preprocess(contract: &ExecutionContract, input: &RuntimeInput) -> ModelResult<Vec<f32>> {
    let declared = &contract.input;
    if input.encoded.len() as u64 > declared.max_bytes
        || input.width > declared.max_width
        || input.height > declared.max_height
        || !declared.accepted_media_types.contains(&input.media_type)
    {
        return Err(ModelError::new(
            "model_input_contract_mismatch",
            "encoded input exceeds the signed execution contract",
        ));
    }
    let decoded = image::load_from_memory(&input.encoded)
        .map_err(|error| runtime_error("decode runtime image input", error))?;
    if decoded.width() != input.width || decoded.height() != input.height {
        return Err(ModelError::new(
            "model_input_contract_mismatch",
            "runtime input metadata does not match decoded image",
        ));
    }
    let height = u32::try_from(declared.tensor.shape[2]).map_err(|_| {
        ModelError::new("model_input_contract_mismatch", "tensor height is invalid")
    })?;
    let width = u32::try_from(declared.tensor.shape[3])
        .map_err(|_| ModelError::new("model_input_contract_mismatch", "tensor width is invalid"))?;
    let pixels = decoded
        .resize_exact(width, height, FilterType::Triangle)
        .to_rgb8();
    let plane = usize::try_from(u64::from(width) * u64::from(height)).map_err(|_| {
        ModelError::new("model_input_contract_mismatch", "input tensor is too large")
    })?;
    let mut values = vec![
        0_f32;
        plane.checked_mul(3).ok_or_else(|| {
            ModelError::new(
                "model_input_contract_mismatch",
                "input tensor size overflow",
            )
        })?
    ];
    for (index, pixel) in pixels.pixels().enumerate() {
        for channel in 0..3 {
            values[channel * plane + index] = (f32::from(pixel[channel])
                * declared.preprocess.scale
                - declared.preprocess.mean[channel])
                / declared.preprocess.std[channel];
        }
    }
    Ok(values)
}

fn validate_session(session: &Session, execution: &ExecutionContract) -> ModelResult<()> {
    if session.inputs().len() != 1
        || !outlet_matches(&session.inputs()[0], &execution.input.tensor)
        || session.outputs().len() != execution.outputs.len()
        || session
            .outputs()
            .iter()
            .zip(&execution.outputs)
            .any(|(outlet, contract)| !outlet_matches(outlet, contract))
    {
        return Err(ModelError::new(
            "model_runtime_contract_mismatch",
            "ONNX session metadata does not match the signed execution contract",
        ));
    }
    Ok(())
}

fn outlet_matches(outlet: &ort::value::Outlet, contract: &TensorContract) -> bool {
    if outlet.name() != contract.name {
        return false;
    }
    match outlet.dtype() {
        ValueType::Tensor { ty, shape, .. } => {
            *ty == TensorElementType::Float32
                && shape.as_ref().iter().all(|dimension| *dimension > 0)
                && shape
                    .as_ref()
                    .iter()
                    .map(|value| *value as u64)
                    .eq(contract.shape.iter().copied())
        }
        _ => false,
    }
}

#[derive(Serialize)]
#[serde(crate = "base::serde")]
struct JsonTensorOutput<'a> {
    name: &'a str,
    dtype: &'static str,
    shape: &'a [u64],
    data: &'a [f32],
}

#[derive(Serialize)]
#[serde(crate = "base::serde")]
struct JsonOutputs<'a> {
    outputs: Vec<JsonTensorOutput<'a>>,
}

fn encode_outputs(
    outputs: &ort::session::SessionOutputs<'_>,
    contracts: &[TensorContract],
    max_result_bytes: usize,
) -> ModelResult<Vec<u8>> {
    let mut extracted = Vec::with_capacity(contracts.len());
    for contract in contracts {
        let value = outputs.get(&contract.name).ok_or_else(|| {
            ModelError::new(
                "model_output_contract_mismatch",
                "required ONNX output is missing",
            )
        })?;
        let (shape, data) = value
            .try_extract_tensor::<f32>()
            .map_err(|error| runtime_error("extract ONNX output tensor", error))?;
        if shape.as_ref().iter().any(|dimension| *dimension <= 0)
            || !shape
                .as_ref()
                .iter()
                .map(|value| *value as u64)
                .eq(contract.shape.iter().copied())
            || data.len() != checked_element_count(&contract.shape)?
        {
            return Err(ModelError::new(
                "model_output_contract_mismatch",
                "ONNX output shape or element count does not match the signed contract",
            ));
        }
        extracted.push(data);
    }
    let estimated =
        contracts
            .iter()
            .zip(&extracted)
            .try_fold(32_usize, |bytes, (contract, data)| {
                bytes
                    .checked_add(contract.name.len())?
                    .checked_add(contract.shape.len().checked_mul(24)?)?
                    .checked_add(data.len().checked_mul(32)?)
            });
    if estimated.is_none_or(|bytes| bytes > max_result_bytes) {
        return Err(ModelError::new(
            "result_too_large",
            "model result exceeds the configured pre-serialization limit",
        ));
    }
    let json = JsonOutputs {
        outputs: contracts
            .iter()
            .zip(extracted)
            .map(|(contract, data)| JsonTensorOutput {
                name: &contract.name,
                dtype: "f32",
                shape: &contract.shape,
                data,
            })
            .collect(),
    };
    let encoded = base::serde_json::to_vec(&json)
        .map_err(|error| ModelError::io("serialize ONNX outputs", error))?;
    if encoded.len() > max_result_bytes {
        return Err(ModelError::new(
            "result_too_large",
            "model result exceeds the configured limit",
        ));
    }
    Ok(encoded)
}

fn media_type_from_path(path: &str) -> ModelResult<&'static str> {
    match Path::new(path).extension().and_then(|value| value.to_str()) {
        Some("png") => Ok("image/png"),
        Some("jpg" | "jpeg") => Ok("image/jpeg"),
        Some("webp") => Ok("image/webp"),
        _ => Err(ModelError::new(
            "model_input_contract_mismatch",
            "self-test input extension is unsupported",
        )),
    }
}

fn runtime_error(stage: &'static str, error: impl std::fmt::Display) -> ModelError {
    ModelError::new("model_runtime_failed", format!("{stage}: {error}"))
}

type NativeJob = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone)]
struct NativeExecutor {
    inner: Arc<NativeExecutorInner>,
}

struct NativeExecutorInner {
    sender: Mutex<Option<mpsc::SyncSender<NativeJob>>>,
    accepting: AtomicBool,
    shutdown: CancellationToken,
    admitted_jobs: AtomicUsize,
    completed_jobs: AtomicUsize,
    active_jobs: AtomicUsize,
    workers_remaining: AtomicUsize,
    workers: Mutex<Option<Vec<JoinHandle<()>>>>,
    state_changed: Notify,
    shutdown_lock: AsyncMutex<()>,
}

impl Drop for NativeExecutorInner {
    fn drop(&mut self) {
        self.accepting.store(false, Ordering::Release);
        self.shutdown.cancel();
        if let Ok(sender) = self.sender.get_mut() {
            sender.take();
        }
    }
}

struct NativeJobGuard {
    inner: Weak<NativeExecutorInner>,
    counted: Arc<AtomicBool>,
}

impl Drop for NativeJobGuard {
    fn drop(&mut self) {
        if self.counted.swap(false, Ordering::AcqRel)
            && let Some(inner) = self.inner.upgrade()
        {
            inner.completed_jobs.fetch_add(1, Ordering::AcqRel);
            inner.active_jobs.fetch_sub(1, Ordering::AcqRel);
            inner.state_changed.notify_one();
        }
    }
}

impl NativeExecutor {
    fn new(worker_count: usize, queue_capacity: usize) -> ModelResult<Self> {
        let (sender, receiver) = mpsc::sync_channel::<NativeJob>(queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let inner = Arc::new(NativeExecutorInner {
            sender: Mutex::new(Some(sender)),
            accepting: AtomicBool::new(true),
            shutdown: CancellationToken::new(),
            admitted_jobs: AtomicUsize::new(0),
            completed_jobs: AtomicUsize::new(0),
            active_jobs: AtomicUsize::new(0),
            workers_remaining: AtomicUsize::new(0),
            workers: Mutex::new(Some(Vec::with_capacity(worker_count))),
            state_changed: Notify::new(),
            shutdown_lock: AsyncMutex::new(()),
        });
        for index in 0..worker_count {
            let receiver = receiver.clone();
            let weak = Arc::downgrade(&inner);
            inner.workers_remaining.fetch_add(1, Ordering::AcqRel);
            let worker = std::thread::Builder::new()
                .name(format!("avai-onnx-cpu-{index}"))
                .spawn(move || {
                    struct WorkerGuard(Weak<NativeExecutorInner>);
                    impl Drop for WorkerGuard {
                        fn drop(&mut self) {
                            if let Some(inner) = self.0.upgrade() {
                                inner.workers_remaining.fetch_sub(1, Ordering::AcqRel);
                                inner.state_changed.notify_one();
                            }
                        }
                    }
                    let _guard = WorkerGuard(weak);
                    loop {
                        let job = match receiver.lock() {
                            Ok(receiver) => receiver.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                })
                .map_err(|error| {
                    inner.workers_remaining.fetch_sub(1, Ordering::AcqRel);
                    ModelError::io("start ONNX native worker", error)
                })?;
            inner
                .workers
                .lock()
                .map_err(|_| {
                    ModelError::new("model_runtime_failed", "native worker lock is poisoned")
                })?
                .as_mut()
                .expect("workers exist during construction")
                .push(worker);
        }
        Ok(Self { inner })
    }

    fn submit(&self, job: NativeJob) -> ModelResult<()> {
        let sender = self.inner.sender.lock().map_err(|_| {
            ModelError::new(
                "model_runtime_failed",
                "native executor sender lock is poisoned",
            )
        })?;
        if !self.inner.accepting.load(Ordering::Acquire) {
            return Err(ModelError::new(
                "model_runtime_unavailable",
                "ONNX native executor is shutting down",
            ));
        }
        let sender = sender.as_ref().ok_or_else(|| {
            ModelError::new(
                "model_runtime_unavailable",
                "ONNX native executor is unavailable",
            )
        })?;
        self.inner.admitted_jobs.fetch_add(1, Ordering::AcqRel);
        self.inner.active_jobs.fetch_add(1, Ordering::AcqRel);
        let counted = Arc::new(AtomicBool::new(true));
        let guard = NativeJobGuard {
            inner: Arc::downgrade(&self.inner),
            counted: counted.clone(),
        };
        sender
            .try_send(Box::new(move || {
                let _guard = guard;
                job();
            }))
            .map_err(|error| {
                if counted.swap(false, Ordering::AcqRel) {
                    self.inner.admitted_jobs.fetch_sub(1, Ordering::AcqRel);
                    self.inner.active_jobs.fetch_sub(1, Ordering::AcqRel);
                }
                match error {
                    mpsc::TrySendError::Full(_) => {
                        ModelError::new("model_runtime_busy", "ONNX native executor queue is full")
                    }
                    mpsc::TrySendError::Disconnected(_) => ModelError::new(
                        "model_runtime_unavailable",
                        "ONNX native executor is unavailable",
                    ),
                }
            })
    }

    async fn close_and_wait(&self, deadline: Instant) -> ModelResult<()> {
        let _shutdown = self.inner.shutdown_lock.lock().await;
        self.inner.accepting.store(false, Ordering::Release);
        self.inner.shutdown.cancel();
        self.inner
            .sender
            .lock()
            .map_err(|_| {
                ModelError::new(
                    "model_runtime_failed",
                    "native executor sender lock is poisoned",
                )
            })?
            .take();
        while self.inner.active_jobs.load(Ordering::Acquire) != 0
            || self.inner.workers_remaining.load(Ordering::Acquire) != 0
        {
            if Instant::now() >= deadline {
                return Err(ModelError::new(
                    "model_runtime_shutdown_incomplete",
                    "ONNX native executor did not drain before its shutdown deadline",
                ));
            }
            base::tokio::select! {
                _ = self.inner.state_changed.notified() => {}
                _ = base::tokio::time::sleep_until(deadline.into()) => {
                    return Err(ModelError::new(
                        "model_runtime_shutdown_incomplete",
                        "ONNX native executor did not drain before its shutdown deadline",
                    ));
                }
            }
        }
        let workers = self
            .inner
            .workers
            .lock()
            .map_err(|_| ModelError::new("model_runtime_failed", "native worker lock is poisoned"))?
            .take()
            .unwrap_or_default();
        for worker in workers {
            worker.join().map_err(|_| {
                ModelError::new(
                    "model_runtime_shutdown_failed",
                    "ONNX native worker panicked",
                )
            })?;
        }
        Ok(())
    }

    async fn execute<T, F, C, E>(
        &self,
        context: RuntimeCallContext,
        cancel: C,
        job: F,
    ) -> ModelResult<T>
    where
        T: Send + 'static,
        F: FnOnce() -> ModelResult<T> + Send + 'static,
        C: FnOnce() -> Result<(), E>,
        E: std::fmt::Display,
    {
        context.ensure_active()?;
        let (result_sender, mut result_receiver) = oneshot::channel();
        self.submit(Box::new(move || {
            let _ = result_sender.send(job());
        }))?;
        let interrupted = base::tokio::select! {
            result = &mut result_receiver => return result.map_err(|_| {
                ModelError::new("model_runtime_unavailable", "ONNX native worker stopped")
            })?,
            _ = context.cancellation.cancelled() => ModelError::new(
                "model_runtime_cancelled",
                "ONNX runtime call was cooperatively cancelled",
            ),
            _ = self.inner.shutdown.cancelled() => ModelError::new(
                "model_runtime_cancelled",
                "ONNX runtime call was cancelled for provider shutdown",
            ),
            _ = base::tokio::time::sleep_until(context.deadline.into()) => ModelError::new(
                "model_runtime_deadline_exceeded",
                "ONNX runtime call exceeded its deadline",
            ),
        };
        let cancel_error = cancel()
            .err()
            .map(|error| runtime_error("signal ONNX cooperative termination", error));
        let _late_result = result_receiver.await.map_err(|_| {
            ModelError::new("model_runtime_unavailable", "ONNX native worker stopped")
        })?;
        if let Some(error) = cancel_error {
            return Err(error);
        }
        Err(interrupted)
    }

    async fn load_session(
        &self,
        context: RuntimeCallContext,
        artifact: PathBuf,
        intra_threads: usize,
        inter_threads: usize,
    ) -> ModelResult<Session> {
        context.ensure_active()?;
        let (canceler_sender, mut canceler_receiver) = oneshot::channel();
        let (result_sender, mut result_receiver) = oneshot::channel();
        self.submit(Box::new(move || {
            let result = (|| {
                let mut builder = Session::builder()
                    .map_err(|error| runtime_error("create ONNX session builder", error))?
                    .with_intra_threads(intra_threads)
                    .map_err(|error| runtime_error("configure ONNX intra-op threads", error))?
                    .with_inter_threads(inter_threads)
                    .map_err(|error| runtime_error("configure ONNX inter-op threads", error))?;
                let canceler = builder.canceler();
                let _ = canceler_sender.send(canceler);
                builder
                    .commit_from_file(&artifact)
                    .map_err(|error| runtime_error("load ONNX model", error))
            })();
            let _ = result_sender.send(result);
        }))?;
        let canceler = base::tokio::select! {
            result = &mut result_receiver => return result.map_err(|_| {
                ModelError::new("model_runtime_unavailable", "ONNX native worker stopped")
            })?,
            canceler = &mut canceler_receiver => canceler.map_err(|_| {
                ModelError::new("model_runtime_unavailable", "ONNX load canceler was unavailable")
            })?,
            _ = context.cancellation.cancelled() => {
                let canceler = canceler_receiver.await.map_err(|_| {
                    ModelError::new("model_runtime_unavailable", "ONNX load canceler was unavailable")
                })?;
                let cancel_error = canceler.cancel()
                    .err()
                    .map(|error| runtime_error("cancel ONNX model load", error));
                let _late_result = result_receiver.await.map_err(|_| {
                    ModelError::new("model_runtime_unavailable", "ONNX native worker stopped")
                })?;
                if let Some(error) = cancel_error {
                    return Err(error);
                }
                return Err(ModelError::new("model_runtime_cancelled", "ONNX model load was cancelled"));
            },
            _ = self.inner.shutdown.cancelled() => {
                let canceler = canceler_receiver.await.map_err(|_| {
                    ModelError::new("model_runtime_unavailable", "ONNX load canceler was unavailable")
                })?;
                let cancel_error = canceler.cancel()
                    .err()
                    .map(|error| runtime_error("cancel ONNX model load for shutdown", error));
                let _late_result = result_receiver.await.map_err(|_| {
                    ModelError::new("model_runtime_unavailable", "ONNX native worker stopped")
                })?;
                if let Some(error) = cancel_error {
                    return Err(error);
                }
                return Err(ModelError::new("model_runtime_cancelled", "ONNX model load was cancelled for provider shutdown"));
            },
            _ = base::tokio::time::sleep_until(context.deadline.into()) => {
                let canceler = canceler_receiver.await.map_err(|_| {
                    ModelError::new("model_runtime_unavailable", "ONNX load canceler was unavailable")
                })?;
                let cancel_error = canceler.cancel()
                    .err()
                    .map(|error| runtime_error("cancel ONNX model load", error));
                let _late_result = result_receiver.await.map_err(|_| {
                    ModelError::new("model_runtime_unavailable", "ONNX native worker stopped")
                })?;
                if let Some(error) = cancel_error {
                    return Err(error);
                }
                return Err(ModelError::new("model_runtime_deadline_exceeded", "ONNX model load exceeded its deadline"));
            },
        };
        let interrupted = base::tokio::select! {
            result = &mut result_receiver => return result.map_err(|_| {
                ModelError::new("model_runtime_unavailable", "ONNX native worker stopped")
            })?,
            _ = context.cancellation.cancelled() => ModelError::new(
                "model_runtime_cancelled",
                "ONNX model load was cancelled",
            ),
            _ = self.inner.shutdown.cancelled() => ModelError::new(
                "model_runtime_cancelled",
                "ONNX model load was cancelled for provider shutdown",
            ),
            _ = base::tokio::time::sleep_until(context.deadline.into()) => ModelError::new(
                "model_runtime_deadline_exceeded",
                "ONNX model load exceeded its deadline",
            ),
        };
        let cancel_error = canceler
            .cancel()
            .err()
            .map(|error| runtime_error("cancel ONNX model load", error));
        let _late_result = result_receiver.await.map_err(|_| {
            ModelError::new("model_runtime_unavailable", "ONNX native worker stopped")
        })?;
        if let Some(error) = cancel_error {
            return Err(error);
        }
        Err(interrupted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_requires_exact_contract_and_cpu_accelerator() {
        let mut selector = super::super::RuntimeVariant {
            runtime: ONNX_CPU_RUNTIME.into(),
            runtime_contract_version: ONNX_CONTRACT_VERSION,
            architecture: std::env::consts::ARCH.into(),
            accelerator: "cpu".into(),
            artifact: "model.onnx".into(),
        };
        validate_onnx_cpu_selector(&selector).unwrap();

        selector.runtime_contract_version += 1;
        assert_eq!(
            validate_onnx_cpu_selector(&selector).unwrap_err().code,
            "model_runtime_contract_unsupported"
        );
        selector.runtime_contract_version = ONNX_CONTRACT_VERSION;
        selector.accelerator = "gpu".into();
        assert_eq!(
            validate_onnx_cpu_selector(&selector).unwrap_err().code,
            "model_accelerator_unavailable"
        );
    }

    #[tokio::test]
    async fn shutdown_reports_incomplete_for_non_cooperative_native_job() {
        let executor = NativeExecutor::new(1, 1).unwrap();
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        executor
            .submit(Box::new(move || {
                started_sender.send(()).unwrap();
                release_receiver.recv().unwrap();
            }))
            .unwrap();
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        let shutdown = base::tokio::time::timeout(
            Duration::from_secs(1),
            executor.close_and_wait(Instant::now() + Duration::from_millis(20)),
        )
        .await;
        release_sender.send(()).unwrap();
        let error = shutdown
            .expect("bounded native shutdown detector hung")
            .unwrap_err();
        assert_eq!(error.code, "model_runtime_shutdown_incomplete");

        executor
            .close_and_wait(Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
    }
}
