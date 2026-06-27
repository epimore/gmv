use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::core::{
    ClockClassifier, ClockState, GuardError, GuardResult, HealthState, LeaseState, NodeIdentity,
    NodeKind, RouteState, SchedulingState,
};
use crate::gateway::{AllocationRequest, AllocationService};
use crate::lease::{LeaseRequest, LeaseService};
use crate::registry::{HeartbeatReport, RegisterRequest, RegistryService};
use crate::route::{ResourceSnapshot, RouteService, SnapshotResource};
use crate::sim::model::{
    EndpointMode, SimAiTask, SimAiTaskState, SimDevice, SimFaults, SimStatus, SimStream,
    SimStreamState,
};
use crate::store::InMemoryGuardStore;
use crate::store::model::RouteRecord;

#[derive(Debug, Clone)]
pub struct Simulator {
    store: InMemoryGuardStore,
    inner: Arc<Mutex<SimInner>>,
}

#[derive(Debug)]
struct SimInner {
    guard_available: bool,
    endpoint_mode: EndpointMode,
    faults: SimFaults,
    devices: Vec<SimDevice>,
    streams: HashMap<String, SimStream>,
    ai_tasks: HashMap<String, SimAiTask>,
    ptz_commands: u64,
}

impl Simulator {
    pub fn new(store: InMemoryGuardStore, endpoint_mode: EndpointMode) -> Self {
        Self {
            store,
            inner: Arc::new(Mutex::new(SimInner {
                guard_available: true,
                endpoint_mode,
                faults: SimFaults::default(),
                devices: vec![SimDevice {
                    device_id: "34020000001320000001".to_string(),
                    name: "模拟摄像机".to_string(),
                    session_node_id: "session-sim-1".to_string(),
                    channels: vec!["ch-1".to_string(), "ch-2".to_string()],
                    online: true,
                }],
                streams: HashMap::new(),
                ai_tasks: HashMap::new(),
                ptz_commands: 0,
            })),
        }
    }

    pub fn bootstrap(&self, now_ms: i64) -> GuardResult<()> {
        let registry = RegistryService::new(self.store.clone());
        for (identity, capabilities, capacity) in [
            (
                NodeIdentity::new("session-sim-1", "session-inst-1", NodeKind::Session),
                vec!["device.control".to_string()],
                32,
            ),
            (
                NodeIdentity::new("stream-sim-1", "stream-inst-1", NodeKind::Stream),
                vec!["live".to_string(), "playback".to_string()],
                8,
            ),
            (
                NodeIdentity::new("avai-sim-1", "avai-inst-1", NodeKind::Avai),
                vec!["ai.vehicle".to_string(), "ai.face".to_string()],
                4,
            ),
        ] {
            registry.register(RegisterRequest {
                identity,
                capabilities,
                endpoints: vec![],
                capacity,
                host_metrics: Default::default(),
                zone: Some("sim-zone".to_string()),
                now_ms,
                takeover: true,
            })?;
        }
        Ok(())
    }

    pub fn store(&self) -> InMemoryGuardStore {
        self.store.clone()
    }

    pub fn devices(&self) -> Vec<SimDevice> {
        self.inner.lock().devices.clone()
    }

    pub fn streams(&self) -> Vec<SimStream> {
        let mut streams = self
            .inner
            .lock()
            .streams
            .values()
            .cloned()
            .collect::<Vec<_>>();
        streams.sort_by(|left, right| left.stream_id.cmp(&right.stream_id));
        streams
    }

    pub fn ai_tasks(&self) -> Vec<SimAiTask> {
        let mut tasks = self
            .inner
            .lock()
            .ai_tasks
            .values()
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| left.task_id.cmp(&right.task_id));
        tasks
    }

    pub fn status(&self) -> SimStatus {
        let inner = self.inner.lock();
        SimStatus {
            guard_available: inner.guard_available,
            streams: inner.streams.len(),
            running_streams: inner
                .streams
                .values()
                .filter(|stream| stream.state == SimStreamState::Running)
                .count(),
            ai_tasks: inner.ai_tasks.len(),
            running_ai_tasks: inner
                .ai_tasks
                .values()
                .filter(|task| task.state == SimAiTaskState::Running)
                .count(),
            ptz_commands: inner.ptz_commands,
        }
    }

    pub fn set_guard_available(&self, available: bool) {
        self.inner.lock().guard_available = available;
    }

    pub fn set_faults(&self, faults: SimFaults) {
        self.inner.lock().faults = faults;
    }

    pub fn heartbeat(
        &self,
        node_id: &str,
        health: HealthState,
        sequence: u64,
        now_ms: i64,
    ) -> GuardResult<()> {
        self.ensure_available()?;
        let identity = self
            .store
            .get_node(node_id)
            .ok_or_else(|| GuardError::NotFound(format!("node {node_id}")))?
            .identity;
        RegistryService::new(self.store.clone()).heartbeat(HeartbeatReport {
            identity,
            health,
            sequence,
            now_ms,
            host_metrics: Default::default(),
            business_metrics: Default::default(),
        })
    }

    pub fn set_clock_offset(&self, node_id: &str, offset_ms: i64) -> GuardResult<ClockState> {
        self.ensure_available()?;
        let state = ClockClassifier::default().classify(offset_ms);
        let mut node = self
            .store
            .get_node(node_id)
            .ok_or_else(|| GuardError::NotFound(format!("node {node_id}")))?;
        node.scheduling = if state == ClockState::TimeUnsynced {
            SchedulingState::TimeUnsynced
        } else if node.health == HealthState::Ready {
            SchedulingState::Enabled
        } else {
            SchedulingState::Disabled
        };
        self.store.upsert_node(node);
        Ok(state)
    }

    pub fn set_capabilities(&self, node_id: &str, capabilities: Vec<String>) -> GuardResult<()> {
        self.ensure_available()?;
        let mut node = self
            .store
            .get_node(node_id)
            .ok_or_else(|| GuardError::NotFound(format!("node {node_id}")))?;
        node.capabilities = capabilities;
        self.store.upsert_node(node);
        Ok(())
    }

    pub fn start_stream(
        &self,
        request_id: &str,
        device_id: &str,
        channel_id: &str,
        now_ms: i64,
    ) -> GuardResult<SimStream> {
        self.ensure_available()?;
        if !self.inner.lock().devices.iter().any(|device| {
            device.device_id == device_id
                && device.online
                && device.channels.iter().any(|channel| channel == channel_id)
        }) {
            return Err(GuardError::NotFound(format!(
                "device/channel {device_id}/{channel_id}"
            )));
        }
        let allocation =
            AllocationService::new(self.store.clone()).allocate(AllocationRequest {
                request_id: request_id.to_string(),
                capability: "live".to_string(),
                zone: Some("sim-zone".to_string()),
            })?;
        let stream_id = format!("stream-{request_id}");
        let lease_id = format!("lease-{request_id}");
        let route_id = format!("route-{request_id}");
        let leases = LeaseService::new(self.store.clone());
        leases.allocate(LeaseRequest {
            lease_id: lease_id.clone(),
            route_id: route_id.clone(),
            resource_id: stream_id.clone(),
            idempotency_key: request_id.to_string(),
            owner: allocation.owner.clone(),
            now_ms,
            ttl_ms: 30_000,
        })?;
        let fail = {
            let mut inner = self.inner.lock();
            let fail = inner.faults.fail_next_stream_start;
            inner.faults.fail_next_stream_start = false;
            fail
        };
        if fail {
            leases.fail(&lease_id, &allocation.owner.instance_id)?;
            return Err(GuardError::Conflict(
                "simulated stream start failure".to_string(),
            ));
        }
        RouteService::new(self.store.clone()).create_allocated(RouteRecord {
            route_id: route_id.clone(),
            resource_id: stream_id.clone(),
            node_id: allocation.owner.node_id.clone(),
            instance_id: allocation.owner.instance_id.clone(),
            state: RouteState::Allocated,
            desired_generation: 1,
            observed_generation: 0,
            observed_sequence: 0,
        })?;
        leases.confirm(&lease_id, &allocation.owner.instance_id)?;
        RouteService::new(self.store.clone()).apply_snapshot(ResourceSnapshot {
            owner: allocation.owner.clone(),
            generation: 1,
            sequence: 1,
            resources: vec![SnapshotResource {
                resource_id: stream_id.clone(),
                route_id: Some(route_id.clone()),
            }],
        })?;
        let endpoint = match self.inner.lock().endpoint_mode {
            EndpointMode::Single => "rtp://127.0.0.1:30000".to_string(),
            EndpointMode::Multi => "rtp://127.0.0.1:30000,30002".to_string(),
        };
        let stream = SimStream {
            stream_id: stream_id.clone(),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            node_id: allocation.owner.node_id,
            instance_id: allocation.owner.instance_id,
            lease_id,
            route_id,
            endpoint,
            state: SimStreamState::Running,
        };
        self.inner.lock().streams.insert(stream_id, stream.clone());
        Ok(stream)
    }

    pub fn stop_stream(&self, stream_id: &str) -> GuardResult<SimStream> {
        self.ensure_available()?;
        let mut inner = self.inner.lock();
        let stream = inner
            .streams
            .get_mut(stream_id)
            .ok_or_else(|| GuardError::NotFound(format!("stream {stream_id}")))?;
        if stream.state != SimStreamState::Running {
            return Err(GuardError::Conflict(format!(
                "stream {stream_id} is not running"
            )));
        }
        LeaseService::new(self.store.clone()).release(&stream.lease_id, &stream.instance_id)?;
        if let Some(mut route) = self.store.get_route(&stream.route_id) {
            route.state = RouteState::Closed;
            self.store.upsert_route(route);
        }
        stream.state = SimStreamState::Stopped;
        Ok(stream.clone())
    }

    pub fn ptz(&self, device_id: &str, channel_id: &str) -> GuardResult<u64> {
        self.ensure_available()?;
        let mut inner = self.inner.lock();
        if !inner.devices.iter().any(|device| {
            device.device_id == device_id
                && device.online
                && device.channels.iter().any(|channel| channel == channel_id)
        }) {
            return Err(GuardError::NotFound(format!(
                "device/channel {device_id}/{channel_id}"
            )));
        }
        inner.ptz_commands += 1;
        Ok(inner.ptz_commands)
    }

    pub fn start_ai(
        &self,
        request_id: &str,
        stream_id: &str,
        model: &str,
        now_ms: i64,
    ) -> GuardResult<SimAiTask> {
        self.ensure_available()?;
        if !self
            .inner
            .lock()
            .streams
            .get(stream_id)
            .is_some_and(|stream| stream.state == SimStreamState::Running)
        {
            return Err(GuardError::NotFound(format!("running stream {stream_id}")));
        }
        let capability = format!("ai.{model}");
        let allocation =
            AllocationService::new(self.store.clone()).allocate(AllocationRequest {
                request_id: request_id.to_string(),
                capability,
                zone: Some("sim-zone".to_string()),
            })?;
        let task_id = format!("ai-{request_id}");
        let lease_id = format!("lease-ai-{request_id}");
        let route_id = format!("route-ai-{request_id}");
        let leases = LeaseService::new(self.store.clone());
        leases.allocate(LeaseRequest {
            lease_id: lease_id.clone(),
            route_id: route_id.clone(),
            resource_id: task_id.clone(),
            idempotency_key: format!("ai-{request_id}"),
            owner: allocation.owner.clone(),
            now_ms,
            ttl_ms: 30_000,
        })?;
        let fail = {
            let mut inner = self.inner.lock();
            let fail = inner.faults.fail_next_ai_start;
            inner.faults.fail_next_ai_start = false;
            fail
        };
        if fail {
            leases.fail(&lease_id, &allocation.owner.instance_id)?;
            return Err(GuardError::Conflict(
                "simulated AI start failure".to_string(),
            ));
        }
        RouteService::new(self.store.clone()).create_allocated(RouteRecord {
            route_id: route_id.clone(),
            resource_id: task_id.clone(),
            node_id: allocation.owner.node_id.clone(),
            instance_id: allocation.owner.instance_id.clone(),
            state: RouteState::Allocated,
            desired_generation: 1,
            observed_generation: 0,
            observed_sequence: 0,
        })?;
        leases.confirm(&lease_id, &allocation.owner.instance_id)?;
        let task = SimAiTask {
            task_id: task_id.clone(),
            model: model.to_string(),
            stream_id: stream_id.to_string(),
            node_id: allocation.owner.node_id,
            instance_id: allocation.owner.instance_id,
            lease_id,
            route_id,
            state: SimAiTaskState::Running,
        };
        self.inner.lock().ai_tasks.insert(task_id, task.clone());
        Ok(task)
    }

    pub fn cancel_ai(&self, task_id: &str) -> GuardResult<SimAiTask> {
        self.ensure_available()?;
        let mut inner = self.inner.lock();
        let task = inner
            .ai_tasks
            .get_mut(task_id)
            .ok_or_else(|| GuardError::NotFound(format!("AI task {task_id}")))?;
        if task.state != SimAiTaskState::Running {
            return Err(GuardError::Conflict(format!(
                "AI task {task_id} is not running"
            )));
        }
        LeaseService::new(self.store.clone()).release(&task.lease_id, &task.instance_id)?;
        if let Some(mut route) = self.store.get_route(&task.route_id) {
            route.state = RouteState::Closed;
            self.store.upsert_route(route);
        }
        task.state = SimAiTaskState::Cancelled;
        Ok(task.clone())
    }

    pub fn reconcile(&self) -> GuardResult<Vec<crate::route::RecoveryIssue>> {
        self.ensure_available()?;
        let inner = self.inner.lock();
        let mut issues = Vec::new();
        for node_id in ["stream-sim-1", "avai-sim-1"] {
            let node = self
                .store
                .get_node(node_id)
                .ok_or_else(|| GuardError::NotFound(format!("node {node_id}")))?;
            let resources = inner
                .streams
                .values()
                .filter(|stream| {
                    stream.node_id == node_id && stream.state == SimStreamState::Running
                })
                .map(|stream| SnapshotResource {
                    resource_id: stream.stream_id.clone(),
                    route_id: Some(stream.route_id.clone()),
                })
                .chain(
                    inner
                        .ai_tasks
                        .values()
                        .filter(|task| {
                            task.node_id == node_id && task.state == SimAiTaskState::Running
                        })
                        .map(|task| SnapshotResource {
                            resource_id: task.task_id.clone(),
                            route_id: Some(task.route_id.clone()),
                        }),
                )
                .collect();
            issues.extend(
                RouteService::new(self.store.clone())
                    .apply_snapshot(ResourceSnapshot {
                        owner: node.identity,
                        generation: 1,
                        sequence: 100,
                        resources,
                    })?
                    .issues,
            );
        }
        Ok(issues)
    }

    pub fn lease_state(&self, lease_id: &str) -> Option<LeaseState> {
        self.store.get_lease(lease_id).map(|lease| lease.state)
    }

    fn ensure_available(&self) -> GuardResult<()> {
        if !self.inner.lock().guard_available {
            return Err(GuardError::Conflict("guard is unavailable".to_string()));
        }
        Ok(())
    }
}
