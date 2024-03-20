use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use common::tokio::sync::Notify;
use common::tokio::time::Instant;
use rsip::SipMessage;
use common::err::TransError;
use common::log::error;
use common::net::shard::Zip;
use common::tokio::sync::mpsc::Sender;
use common::tokio::time;
use common::once_cell::sync::Lazy;
use common::tokio;

static QUEUE: Lazy<Queue<Actor>> = Lazy::new(|| Queue::init());

//延时队列封装
struct Queue<T: Processor + Send + 'static> {
    shared: Arc<Shared<T>>,
}

impl<T: Processor + Send + 'static> Queue<T> {
    pub fn init() -> Self {
        let session = Queue {
            shared: Arc::new(
                Shared {
                    state: Mutex::new(
                        State {
                            expirations: BTreeMap::new(),
                        }
                    ),
                    background_task: Notify::new(),
                }
            )
        };
        let shared = session.shared.clone();

        thread::spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().hand_err(|msg| error!("{msg}")).unwrap();
            rt.block_on(Self::purge_expired_tasks(shared));
        });
        session
    }

    async fn purge_expired_tasks(shared: Arc<Shared<T>>) {
        loop {
            if let Some(when) = shared.purge_expired_keys().await {
                tokio::select! {
                    _ = time::sleep_until(when) =>{},
                    _ = shared.background_task.notified()=>{}
                }
            } else {
                shared.background_task.notified().await;
            }
        }
    }
}

struct Shared<T: Processor + Send> {
    state: Mutex<State<T>>,
    background_task: Notify,
}

impl<T: Processor + Send> Shared<T> {
    async fn purge_expired_keys(&self) -> Option<Instant> {
        let now = Instant::now();
        let mut gstate = self.state.lock().unwrap();
        let state = &mut *gstate;
        while let Some(((when, key), actor)) = state.expirations.iter().next() {
            if when > &now {
                return Some(*when);
            }
            actor.act().await;
            state.expirations.remove(&(*when, key.to_owned()));
        }
        None
    }
}

struct State<T: Processor + Send> {
    expirations: BTreeMap<(Instant, String), T>,
}

impl<T: Processor + Send> State<T> {
    fn next_expiration(&self) -> Option<Instant> {
        self.expirations.keys().next().map(|expiration| expiration.0)
    }
}

pub trait Processor {
    async fn act(&self);
}

pub enum Actor {
    Register(String, Sender<Zip>)
}

impl Processor for Actor {
    async fn act(&self) {
        match self {
            Actor::Register(device_id, sender) => {
                // handle_register_next(did.to_string(), *addr, sender.clone()).await
                //     .unwrap_or_else(|err| error!("handle_register_next:err ={}",err));
            }
        }
    }
}

pub fn insert(key: &str, expires: Duration, data: Actor) {
    let mut state = QUEUE.shared.state.lock().unwrap();
    let when = Instant::now() + expires;
    let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
    state.expirations.insert((when, key.to_string()), data);
    drop(state);
    if notify {
        QUEUE.shared.background_task.notify_one();
    }
}