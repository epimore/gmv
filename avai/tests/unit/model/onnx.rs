
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
