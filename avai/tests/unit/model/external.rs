use super::*;

#[test]
fn external_v1_requires_exact_contract_and_unclaimed_accelerator() {
    let mut selector = super::super::RuntimeVariant {
        runtime: "external-test".into(),
        runtime_contract_version: 7,
        architecture: std::env::consts::ARCH.into(),
        accelerator: String::new(),
        artifact: "model.bin".into(),
    };
    validate_external_selector(&selector, 7).unwrap();

    selector.runtime_contract_version = 8;
    assert_eq!(
        validate_external_selector(&selector, 7).unwrap_err().code,
        "model_runtime_contract_unsupported"
    );
    selector.runtime_contract_version = 7;
    selector.accelerator = "gpu".into();
    assert_eq!(
        validate_external_selector(&selector, 7).unwrap_err().code,
        "model_accelerator_unavailable"
    );
}
