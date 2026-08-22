pub(crate) mod packetizer;
mod session;

pub(crate) use session::BroadcastManager;

#[allow(dead_code)]
pub(crate) const MAX_BROADCAST_PARENTS_PER_NODE: usize = 8;
#[allow(dead_code)]
pub(crate) const MAX_BROADCAST_LEGS_PER_PARENT: usize = 50;
#[allow(dead_code)]
pub(crate) const MAX_BROADCAST_LEGS_PER_NODE: usize = 50;
