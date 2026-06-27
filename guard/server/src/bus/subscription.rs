use crate::bus::queue::BoundedQueue;
use crate::bus::service::BusEvent;

#[derive(Debug, Clone)]
pub struct Subscription {
    pub pattern: String,
    pub queue: BoundedQueue<BusEvent>,
}
