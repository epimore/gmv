use crate::api::v2::events::{EventPage, EventQuery, poll_events};
use crate::core::GuardResult;
use crate::job::{SystemJobRecord, SystemJobRequest, SystemJobService};
use crate::operation::{OperationRecord, OperationRequest, OperationService};
use crate::store::InMemoryGuardStore;
use crate::store::model::{LeaseRecord, NodeRecord};

#[derive(Debug, Clone)]
pub struct ApiV2 {
    store: InMemoryGuardStore,
    operations: OperationService,
    jobs: SystemJobService,
}

impl ApiV2 {
    pub fn new(
        store: InMemoryGuardStore,
        operations: OperationService,
        jobs: SystemJobService,
    ) -> Self {
        Self {
            store,
            operations,
            jobs,
        }
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

    pub fn get_operation(&self, operation_id: &str) -> GuardResult<OperationRecord> {
        self.operations.get(operation_id)
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

    pub fn list_operations(&self) -> Vec<OperationRecord> {
        self.operations.list()
    }

    pub fn start_system_job(&self, request: SystemJobRequest) -> GuardResult<SystemJobRecord> {
        self.jobs.start(request)
    }

    pub fn get_system_job(&self, job_id: &str) -> GuardResult<SystemJobRecord> {
        self.jobs.get(job_id)
    }

    pub fn list_system_jobs(&self) -> Vec<SystemJobRecord> {
        self.jobs.list()
    }
}
