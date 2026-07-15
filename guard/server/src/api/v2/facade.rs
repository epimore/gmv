use crate::api::v2::events::{EventPage, EventQuery, poll_events};
use crate::core::GuardResult;
use crate::operation::{OperationRecord, OperationRequest, OperationService};
use crate::store::InMemoryGuardStore;
use crate::store::model::{LeaseRecord, NodeRecord};

#[derive(Debug, Clone)]
pub struct ApiV2 {
    store: InMemoryGuardStore,
    operations: OperationService,
}

impl ApiV2 {
    pub fn new(store: InMemoryGuardStore, operations: OperationService) -> Self {
        Self { store, operations }
    }

    pub fn store(&self) -> InMemoryGuardStore {
        self.store.clone()
    }

    pub fn list_nodes(&self) -> Vec<NodeRecord> {
        self.store.nodes()
    }

    pub fn list_leases(&self) -> Vec<LeaseRecord> {
        self.store.leases()
    }

    pub fn poll_events(&self, query: EventQuery) -> GuardResult<EventPage> {
        poll_events(&self.store, query)
    }

    pub fn start_operation(&self, request: OperationRequest) -> GuardResult<OperationRecord> {
        self.operations.start(request)
    }

    pub fn start_operation_once(
        &self,
        request: OperationRequest,
    ) -> GuardResult<(OperationRecord, bool)> {
        self.operations.start_once(request)
    }

    pub fn get_operation(&self, operation_id: &str) -> GuardResult<OperationRecord> {
        self.operations.get(operation_id)
    }

    pub fn list_operations(&self) -> Vec<OperationRecord> {
        self.operations.list()
    }

    pub fn configure_media_operation(
        &self,
        operation_id: &str,
        stage: impl Into<String>,
        checkpoint_ms: u64,
        hard_timeout_ms: u64,
    ) -> GuardResult<OperationRecord> {
        self.operations
            .configure_media(operation_id, stage, checkpoint_ms, hard_timeout_ms)
    }

    pub fn progress_operation(
        &self,
        operation_id: &str,
        stage: impl Into<String>,
        progress_percent: u8,
        message: impl Into<String>,
    ) -> GuardResult<OperationRecord> {
        self.operations
            .progress_stage(operation_id, stage, progress_percent, message)
    }

    pub fn succeed_operation(
        &self,
        operation_id: &str,
        message: impl Into<String>,
    ) -> GuardResult<OperationRecord> {
        self.operations.succeed(operation_id, message)
    }

    pub fn fail_operation(
        &self,
        operation_id: &str,
        error: crate::core::GuardError,
    ) -> GuardResult<OperationRecord> {
        self.operations.fail(operation_id, error)
    }

    pub fn succeed_operation_with_result(
        &self,
        operation_id: &str,
        message: impl Into<String>,
        result: base::serde_json::Value,
    ) -> GuardResult<OperationRecord> {
        self.operations
            .succeed_with_result(operation_id, message, result)
    }

    pub fn cancel_operation(&self, operation_id: &str) -> GuardResult<OperationRecord> {
        self.operations.cancel(operation_id)
    }
}
