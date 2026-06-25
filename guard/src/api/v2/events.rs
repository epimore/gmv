use crate::api::v2::page::{CursorPage, CursorQuery};
use crate::core::GuardResult;
use crate::store::InMemoryGuardStore;
use crate::store::model::EventRecord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventQuery {
    pub cursor: CursorQuery,
    pub topic_prefix: Option<String>,
    pub min_priority: Option<u8>,
}

impl Default for EventQuery {
    fn default() -> Self {
        Self {
            cursor: CursorQuery::default(),
            topic_prefix: None,
            min_priority: None,
        }
    }
}

pub type EventPage = CursorPage<EventRecord>;

pub fn poll_events(store: &InMemoryGuardStore, query: EventQuery) -> GuardResult<EventPage> {
    query.cursor.validate()?;
    let limit = query.cursor.limit;
    let mut items = store.events_after(query.cursor.after_id.as_deref(), limit);
    if let Some(prefix) = query.topic_prefix {
        items.retain(|event| event.topic.starts_with(&prefix));
    }
    if let Some(min_priority) = query.min_priority {
        items.retain(|event| event.priority >= min_priority);
    }
    items.truncate(limit);
    let next_after_id = items.last().map(|event| event.event_id.clone());
    Ok(EventPage {
        items,
        next_after_id,
    })
}
