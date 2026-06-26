use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::core::{GuardError, GuardResult};
use crate::job::state::{SystemJobRecord, SystemJobStatus, SystemJobType};

#[derive(Debug, Clone)]
pub struct SystemJobRequest {
    pub job_id: String,
    pub job_type: SystemJobType,
}

#[derive(Debug, Clone, Default)]
pub struct SystemJobService {
    records: Arc<Mutex<HashMap<String, SystemJobRecord>>>,
}

impl SystemJobService {
    pub fn start(&self, request: SystemJobRequest) -> GuardResult<SystemJobRecord> {
        if request.job_id.is_empty() {
            return Err(GuardError::InvalidConfig("job_id is required".to_string()));
        }
        let record = SystemJobRecord {
            job_id: request.job_id,
            job_type: request.job_type,
            status: SystemJobStatus::Pending,
            progress_percent: 0,
            message: String::new(),
            error: None,
        };
        let mut records = self.records.lock();
        if records.contains_key(&record.job_id) {
            return Err(GuardError::Conflict(format!(
                "system job {} already exists",
                record.job_id
            )));
        }
        records.insert(record.job_id.clone(), record.clone());
        Ok(record)
    }

    pub fn progress(
        &self,
        job_id: &str,
        progress_percent: u8,
        message: impl Into<String>,
    ) -> GuardResult<SystemJobRecord> {
        if progress_percent > 100 {
            return Err(GuardError::InvalidConfig(
                "job progress must be <= 100".to_string(),
            ));
        }
        self.update(job_id, |record| {
            if record.status.is_terminal() {
                return Err(GuardError::Conflict(format!(
                    "system job {job_id} is terminal"
                )));
            }
            record.status = SystemJobStatus::Running;
            record.progress_percent = progress_percent;
            record.message = message.into();
            Ok(())
        })
    }

    pub fn succeed(
        &self,
        job_id: &str,
        message: impl Into<String>,
    ) -> GuardResult<SystemJobRecord> {
        self.update(job_id, |record| {
            if record.status.is_terminal() {
                return Err(GuardError::Conflict(format!(
                    "system job {job_id} is terminal"
                )));
            }
            record.status = SystemJobStatus::Succeeded;
            record.progress_percent = 100;
            record.message = message.into();
            Ok(())
        })
    }

    pub fn fail(&self, job_id: &str, error: GuardError) -> GuardResult<SystemJobRecord> {
        self.update(job_id, |record| {
            if record.status.is_terminal() {
                return Err(GuardError::Conflict(format!(
                    "system job {job_id} is terminal"
                )));
            }
            record.status = SystemJobStatus::Failed;
            record.error = Some(error);
            Ok(())
        })
    }

    pub fn get(&self, job_id: &str) -> GuardResult<SystemJobRecord> {
        self.records
            .lock()
            .get(job_id)
            .cloned()
            .ok_or_else(|| GuardError::NotFound(format!("system job {job_id}")))
    }

    pub fn list(&self) -> Vec<SystemJobRecord> {
        let mut records = self.records.lock().values().cloned().collect::<Vec<_>>();
        records.sort_by(|left, right| left.job_id.cmp(&right.job_id));
        records
    }

    fn update(
        &self,
        job_id: &str,
        update: impl FnOnce(&mut SystemJobRecord) -> GuardResult<()>,
    ) -> GuardResult<SystemJobRecord> {
        let mut records = self.records.lock();
        let record = records
            .get_mut(job_id)
            .ok_or_else(|| GuardError::NotFound(format!("system job {job_id}")))?;
        update(record)?;
        Ok(record.clone())
    }
}
