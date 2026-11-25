use base::{
    base64::{Engine, engine::general_purpose::STANDARD},
    serde_json::{Value, json},
    sha2::{Digest, Sha256},
};
use ed25519_dalek::{Signer, SigningKey};
use gmv_model_package::*;
use ort::{
    session::Session,
    value::{Tensor, TensorElementType, ValueType},
};
use std::{
    collections::HashMap,
    error::Error,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

const MODEL_SHA: &str = "1eeff551a67ae8d565ca33b572fc4b66e3ef357b0eb2863bb9ff47a918cc4088";
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn outlet(value: &ort::value::Outlet, layout: &str) -> Result<TensorContract, Box<dyn Error>> {
    match value.dtype() {
        ValueType::Tensor { ty, shape, .. }
            if *ty == TensorElementType::Float32 && shape.as_ref().iter().all(|dim| *dim > 0) =>
        {
            Ok(TensorContract {
                name: value.name().into(),
                dtype: "f32".into(),
                shape: shape.as_ref().iter().map(|dim| *dim as u64).collect(),
                layout: layout.into(),
            })
        }
        _ => Err("model must have static f32 tensors".into()),
    }
}
fn main() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: open_model_bundle <model.onnx> <ORT-library> <output-dir>".into());
    }
    let model_path = Path::new(&args[0]);
    let library = Path::new(&args[1]);
    let root = PathBuf::from(&args[2]);
    let model = fs::read(model_path)?;
    if sha(&model) != MODEL_SHA {
        return Err("SqueezeNet model SHA-256 mismatch".into());
    }
    ort::init_from(library)?
        .with_name("open-model-bundle-smoke")
        .with_telemetry(false)
        .commit();
    let build = ort::info().to_string();
    if !build.contains("git-commit-id=da9b5e364c") {
        return Err("ORT is not the approved 1.28.0 build".into());
    }
    let mut session = Session::builder()?
        .with_intra_threads(1)?
        .with_inter_threads(1)?
        .commit_from_file(model_path)?;
    if session.inputs().len() != 1 || session.outputs().is_empty() {
        return Err("expected one input and at least one output".into());
    }
    let input = outlet(&session.inputs()[0], "nchw")?;
    if input.shape != [1, 3, 224, 224] {
        return Err(format!("unexpected input shape: {:?}", input.shape).into());
    }
    let outputs = session
        .outputs()
        .iter()
        .map(|value| outlet(value, ""))
        .collect::<Result<Vec<_>, _>>()?;
    let image_bytes = include_bytes!("../tests/fixtures/onnx_cpu/input.png");
    let decoded = image::load_from_memory(image_bytes)?;
    let pixels = decoded
        .resize_exact(224, 224, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let mean = [0.485_f32, 0.456, 0.406];
    let std = [0.229_f32, 0.224, 0.225];
    let mut data = vec![0_f32; 3 * 224 * 224];
    for (index, pixel) in pixels.pixels().enumerate() {
        for channel in 0..3 {
            data[channel * 224 * 224 + index] =
                (f32::from(pixel[channel]) * (1.0 / 255.0) - mean[channel]) / std[channel];
        }
    }
    let tensor = Tensor::from_array((vec![1_usize, 3, 224, 224], data.into_boxed_slice()))?;
    let result = session.run(vec![(input.name.as_str(), tensor)])?;
    let mut values = Vec::<Value>::new();
    for contract in &outputs {
        let (shape, data) = result
            .get(&contract.name)
            .ok_or("missing declared output")?
            .try_extract_tensor::<f32>()?;
        if shape
            .as_ref()
            .iter()
            .map(|dim| *dim as u64)
            .collect::<Vec<_>>()
            != contract.shape
            || data.iter().any(|value| !value.is_finite())
        {
            return Err("invalid inference output".into());
        }
        values.push(json!({"name":contract.name,"dtype":"f32","shape":contract.shape,"data":data}));
    }
    let expected = base::serde_json::to_vec_pretty(&json!({"outputs":values}))?;
    let schema = br#"{"type":"object","required":["outputs"]}"#.to_vec();
    let files = vec![
        ("model/model.onnx", model),
        ("tests/input.png", image_bytes.to_vec()),
        ("tests/expected.json", expected),
        ("schema/result.schema.json", schema),
    ];
    let key = SigningKey::from_bytes(&[7_u8; 32]);
    let mut manifest = ModelPackageManifest {
        api_version: MODEL_PACKAGE_API_VERSION.into(),
        kind: MODEL_PACKAGE_KIND.into(),
        metadata: ModelIdentity {
            model_id: "open-squeezenet".into(),
            version: "1".into(),
            revision: "r1".into(),
        },
        capabilities: vec!["tensor.test".into()],
        result_schema: ResultSchema {
            name: "gmv.tensor.outputs".into(),
            version: 1,
            path: "schema/result.schema.json".into(),
        },
        variants: vec![RuntimeVariant {
            runtime: "onnx-cpu".into(),
            runtime_contract_version: 1,
            architecture: "x86_64".into(),
            accelerator: "cpu".into(),
            artifact: "model/model.onnx".into(),
        }],
        execution: Some(ExecutionContract {
            version: 1,
            input: ExecutionInput {
                kind: "encoded_image_tensor_v1".into(),
                accepted_media_types: vec!["image/png".into()],
                max_bytes: 1024,
                max_width: decoded.width(),
                max_height: decoded.height(),
                tensor: input.clone(),
                preprocess: PreprocessContract {
                    resize: "exact".into(),
                    interpolation: "bilinear".into(),
                    color: "rgb".into(),
                    scale: 1.0 / 255.0,
                    mean,
                    std,
                },
            },
            outputs: outputs.clone(),
            postprocess: PostprocessContract {
                kind: "tensor_json_v1".into(),
            },
        }),
        resources: ResourceHints {
            memory_mb: 512,
            vram_mb: 0,
            max_batch: 1,
        },
        license: LicenseSpec {
            spdx: "Apache-2.0".into(),
            commercial_use: true,
            redistribution: "allowed".into(),
            license_ref: String::new(),
        },
        self_test: vec![SelfTestCase {
            input: "tests/input.png".into(),
            expected: "tests/expected.json".into(),
            oracle: Some(SelfTestOracle {
                kind: "json_numeric_v1".into(),
                abs_tolerance: 0.0001,
                rel_tolerance: 0.0001,
            }),
        }],
        files: files
            .iter()
            .map(|(path, bytes)| ModelFile {
                path: (*path).into(),
                sha256: sha(bytes),
                size: bytes.len() as u64,
            })
            .collect(),
        signing: SigningSpec {
            key_id: "smoke-key".into(),
            signature: String::new(),
        },
    };
    manifest.signing.signature = STANDARD.encode(
        key.sign(&model_package_signing_payload(&manifest)?)
            .to_bytes(),
    );
    let manifest_bytes = base::serde_yaml::to_string(&manifest)?.into_bytes();
    let mut archive = tar::Builder::new(Vec::new());
    for (path, bytes) in std::iter::once(("manifest.yaml", &manifest_bytes))
        .chain(files.iter().map(|(path, bytes)| (*path, bytes)))
    {
        let mut header = tar::Header::new_gnu();
        header.set_path(path)?;
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_cksum();
        archive.append(&header, Cursor::new(bytes))?;
    }
    let bundle = archive.into_inner()?;
    let keys = HashMap::from([("smoke-key".into(), key.verifying_key().to_bytes().to_vec())]);
    verify_model_bundle(Cursor::new(&bundle), &keys, BundleLimits::default())?;
    fs::create_dir_all(&root)?;
    for (path, bytes) in &files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().ok_or("invalid output path")?)?;
        fs::write(path, bytes)?;
    }
    fs::write(root.join("manifest.yaml"), &manifest_bytes)?;
    fs::write(root.join("model.bundle.tar"), &bundle)?;
    fs::write(
        root.join("public-key.txt"),
        format!("{}\n", STANDARD.encode(key.verifying_key().to_bytes())),
    )?;
    let resources = json!({"model_source":"https://huggingface.co/onnxmodelzoo/squeezenet1.1-7/resolve/61e525224ad479521059f4586bcacf50ad3627ca/squeezenet1.1-7.onnx","source_revision":"61e525224ad479521059f4586bcacf50ad3627ca","model_sha256":MODEL_SHA,"model_size_bytes":files[0].1.len(),"bundle_sha256":sha(&bundle),"bundle_size_bytes":bundle.len(),"manifest_sha256":sha(&manifest_bytes),"runtime_sha256":sha(&fs::read(library)?),"runtime_build_info":build,"input":input,"outputs":outputs,"signing_key_id":"smoke-key","test_only_key":true,"trained_or_finetuned":false});
    fs::write(
        root.join("resources.json"),
        base::serde_json::to_vec_pretty(&resources)?,
    )?;
    println!("{}", base::serde_json::to_string_pretty(&resources)?);
    Ok(())
}
