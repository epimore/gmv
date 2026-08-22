use crate::store::{InMemoryGuardStore, model::EventRecord};

#[derive(Debug, Clone)]
pub struct EventLog {
    store: InMemoryGuardStore,
}

impl EventLog {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self { store }
    }

    pub fn append_once(&self, event: EventRecord) -> bool {
        self.store.insert_event_once(event).unwrap_or(false)
    }
}
