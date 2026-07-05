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
}
