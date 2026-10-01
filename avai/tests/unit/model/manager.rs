use super::{LoadFailureEvidence, ModelError, classify_load_failure};

#[test]
fn load_failure_classification_separates_runtime_availability_from_model_evidence() {
    for code in [
        "model_runtime_provider_unavailable",
        "model_runtime_stale_handle",
        "model_runtime_busy",
        "model_runtime_deadline_exceeded",
        "model_runtime_cancelled",
        "model_runtime_protocol_mismatch",
        "model_runtime_protocol_violation",
        "model_runtime_response_invalid",
    ] {
        assert!(matches!(
            classify_load_failure(&ModelError::new(code, "test")),
            LoadFailureEvidence::RuntimeAvailability
        ));
    }
    for code in [
        "model_runtime_contract_mismatch",
        "model_preload_failed",
        "model_self_test_failed",
        "model_health_failed",
    ] {
        assert!(matches!(
            classify_load_failure(&ModelError::new(code, "test")),
            LoadFailureEvidence::IntrinsicModel
        ));
    }
}
