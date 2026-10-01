use super::*;

#[cfg(test)]
#[path = "../../../tests/unit/model/onnx_executor.rs"]
mod tests;

type NativeJob = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone)]
pub(super) struct NativeExecutor {
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
    pub(super) fn new(worker_count: usize, queue_capacity: usize) -> ModelResult<Self> {
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

    pub(super) async fn close_and_wait(&self, deadline: Instant) -> ModelResult<()> {
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

    pub(super) async fn execute<T, F, C, E>(
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

    pub(super) async fn load_session(
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

impl OnnxCpuProvider {
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
