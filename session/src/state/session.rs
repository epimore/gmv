use std::collections::hash_map::Entry::{Occupied, Vacant};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::register::core::Register;
use crate::state;
use base::dashmap::mapref::entry::Entry;
use base::dashmap::{DashMap, DashSet};
use base::log::warn;
use base::once_cell::sync::Lazy;
use base::rand;
use base::rand::seq::IteratorRandom;
use base::tokio::sync::Mutex as AsyncMutex;
use base::tokio::sync::mpsc::Sender;
use base::tokio::time::Instant;
use shared::info::media_info::MediaConfig;
use shared::info::obj::BaseStreamInfo;

static GENERAL_CACHE: Lazy<Cache> = Lazy::new(Cache::init);

pub struct Cache {
    shared: Arc<Shared>,
}

#[derive(Clone)]
pub struct DeviceStreamState {
    pub channel_id: String,
    pub stream_id: String,
    pub ssrc: String,
    pub call_id: String,
    pub seq: u32,
    pub from_tag: String,
    pub to_tag: String,
}

impl Cache {
    pub fn ssrc_sn_get() -> Option<u16> {
        let mut rng = rand::thread_rng();
        if let Some(val) = GENERAL_CACHE
            .shared
            .ssrc_sn
            .iter()
            .choose(&mut rng)
            .map(|v| *v)
        {
            return GENERAL_CACHE.shared.ssrc_sn.remove(&val);
        }
        None
    }

    pub fn ssrc_sn_set(ssrc_sn: u16) -> bool {
        GENERAL_CACHE.shared.ssrc_sn.insert(ssrc_sn)
    }

    pub fn stream_map_order_node() -> BTreeSet<(u16, String)> {
        let mut map = HashMap::<String, u16>::new();
        let mut dash_iter = GENERAL_CACHE.shared.stream_map.iter();
        while let Some(item) = dash_iter.next() {
            let node_name = item.value().stream_node_name.clone();
            match map.entry(node_name) {
                Occupied(mut occ) => {
                    *occ.get_mut() += 1;
                }
                Vacant(vac) => {
                    vac.insert(1);
                }
            }
        }
        let mut set = BTreeSet::new();
        let conf = state::StreamConf::get_stream_conf();
        for (k, _v) in conf.node_map.iter() {
            let count = map.get(k).unwrap_or(&0);
            set.insert((*count, k.clone()));
        }
        set
    }

    pub fn stream_map_insert_token(stream_id: String, gmv_token: String) -> bool {
        match GENERAL_CACHE.shared.stream_map.entry(stream_id) {
            Entry::Occupied(mut occ) => {
                occ.get_mut().gmv_token_sets.insert(gmv_token);
                true
            }
            Entry::Vacant(_) => false,
        }
    }

    pub fn stream_map_insert_info(
        stream_id: String,
        ssrc: u32,
        proxy_addr: String,
        stream_node_name: String,
        call_id: String,
        seq: u32,
        am: AccessMode,
        from_tag: String,
        to_tag: String,
    ) -> bool {
        match GENERAL_CACHE.shared.stream_map.entry(stream_id) {
            Entry::Occupied(_) => false,
            Entry::Vacant(vac) => {
                vac.insert(StreamTable {
                    gmv_token_sets: HashSet::new(),
                    proxy_addr,
                    stream_node_name,
                    call_id,
                    seq,
                    am,
                    from_tag,
                    to_tag,
                    ssrc,
                });
                true
            }
        }
    }

    pub fn stream_map_query_node_ssrc(stream_id: &String) -> Option<(String, u32)> {
        GENERAL_CACHE.shared.stream_map.get(stream_id).map(|item| {
            let node_name = item.stream_node_name.clone();
            (node_name, item.ssrc)
        })
    }

    pub fn stream_map_query_node(stream_id: &String) -> Option<(String, String)> {
        GENERAL_CACHE.shared.stream_map.get(stream_id).map(|item| {
            (
                item.value().stream_node_name.clone(),
                item.value().proxy_addr.clone(),
            )
        })
    }

    pub fn stream_map_remove(stream_id: &String, gmv_token: Option<&String>) {
        match gmv_token {
            None => {
                GENERAL_CACHE.shared.stream_map.remove(stream_id);
            }
            Some(token) => match GENERAL_CACHE.shared.stream_map.entry(stream_id.to_string()) {
                Entry::Occupied(mut occ) => {
                    occ.get_mut().gmv_token_sets.remove(token);
                }
                Entry::Vacant(_) => {}
            },
        }
    }

    pub fn stream_map_contains_token(stream_id: &String, gmv_token: &String) -> bool {
        match GENERAL_CACHE.shared.stream_map.get(stream_id) {
            None => false,
            Some(inner_ref) => inner_ref.value().gmv_token_sets.contains(gmv_token),
        }
    }

    pub fn stream_map_build_call_id_seq_from_to_tag(
        stream_id: &String,
    ) -> Option<(String, u32, String, String)> {
        GENERAL_CACHE
            .shared
            .stream_map
            .get_mut(stream_id)
            .map(|mut ref_mut| {
                let stream_table = ref_mut.value_mut();
                stream_table.seq += 1;
                (
                    stream_table.call_id.clone(),
                    stream_table.seq,
                    stream_table.from_tag.clone(),
                    stream_table.to_tag.clone(),
                )
            })
    }

    pub fn stream_map_query_play_type_by_stream_id(stream_id: &String) -> Option<AccessMode> {
        GENERAL_CACHE
            .shared
            .stream_map
            .get(stream_id)
            .map(|res| res.value().am)
    }

    pub fn device_map_insert(
        device_id: String,
        channel_id: String,
        ssrc: String,
        stream_id: String,
        am: AccessMode,
        config: MediaConfig,
    ) {
        let device_table = DeviceTable {
            channel_id,
            am,
            stream_id,
            config,
            ssrc,
        };
        match GENERAL_CACHE.shared.device_map.entry(device_id) {
            Entry::Occupied(mut occ) => {
                occ.get_mut().push(device_table);
            }
            Entry::Vacant(vac) => {
                vac.insert(vec![device_table]);
            }
        }
    }

    pub fn device_map_remove(
        device_id: &String,
        opt_channel_ssrc: Option<(&String, Option<(AccessMode, &String)>)>,
    ) {
        match opt_channel_ssrc {
            None => {
                GENERAL_CACHE.shared.device_map.remove(device_id);
            }
            Some((channel_id, channel_ssrc)) => {
                match GENERAL_CACHE.shared.device_map.entry(device_id.to_string()) {
                    Entry::Occupied(mut m_occ) => {
                        let s_vec = m_occ.get_mut();
                        s_vec.retain(|device_table| match channel_ssrc {
                            None => !device_table.channel_id.eq(channel_id),
                            Some((am, ssrc)) => {
                                device_table.channel_id != *channel_id
                                    || device_table.am != am
                                    || device_table.ssrc != *ssrc
                            }
                        });
                        if s_vec.is_empty() {
                            m_occ.remove();
                        }
                    }
                    Entry::Vacant(_) => {}
                }
            }
        }
    }

    pub fn device_map_get_invite_info(
        device_id: &String,
        channel_id: &String,
        am: &AccessMode,
    ) -> Option<(String, String)> {
        match GENERAL_CACHE.shared.device_map.get(device_id) {
            None => None,
            Some(m_map) => m_map.value().iter().find_map(|device_table| {
                if device_table.channel_id.eq(channel_id) && device_table.am.eq(am) {
                    Some((device_table.stream_id.clone(), device_table.ssrc.clone()))
                } else {
                    None
                }
            }),
        }
    }

    pub fn stream_setup_lock(
        device_id: &str,
        channel_id: &str,
        am: AccessMode,
    ) -> Arc<AsyncMutex<()>> {
        let key = format!("{}:{}:{}", device_id, channel_id, am.as_str());
        match GENERAL_CACHE.shared.stream_setup_locks.entry(key) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let lock = Arc::new(AsyncMutex::new(()));
                entry.insert(lock.clone());
                lock
            }
        }
    }

    pub fn reset_device_state(device_id: &str) -> Vec<DeviceStreamState> {
        let mut streams = Vec::new();
        if let Some((_, entries)) = GENERAL_CACHE.shared.device_map.remove(device_id) {
            for entry in entries {
                if let Some((_, stream)) = GENERAL_CACHE.shared.stream_map.remove(&entry.stream_id)
                {
                    let ssrc_num = (stream.ssrc % 10000) as u16;
                    GENERAL_CACHE.shared.ssrc_sn.insert(ssrc_num);
                    streams.push(DeviceStreamState {
                        channel_id: entry.channel_id,
                        stream_id: entry.stream_id,
                        ssrc: entry.ssrc,
                        call_id: stream.call_id,
                        seq: stream.seq.saturating_add(1),
                        from_tag: stream.from_tag,
                        to_tag: stream.to_tag,
                    });
                }
            }
        }
        streams
    }

    fn upsert_state(
        key: String,
        value: StateValue,
        expire: Option<Instant>,
        waiter: Option<StateWaiter>,
    ) {
        let mut guard = GENERAL_CACHE.shared.state.lock();
        guard.entities.insert(
            key.clone(),
            StateEntry {
                value,
                expire,
                waiter,
            },
        );
        drop(guard);
        if let Some(when) = expire {
            let ttl = when
                .checked_duration_since(Instant::now())
                .unwrap_or_default();
            let _ = Register::scheduler().insert_general_cache(key, ttl);
        } else {
            let _ = Register::scheduler().remove_general_cache(&key);
        }
    }

    pub fn insert_stream_wait(key: String, expire: Instant, tx: Sender<Option<BaseStreamInfo>>) {
        Self::upsert_state(
            key,
            StateValue::Empty,
            Some(expire),
            Some(StateWaiter::Stream(tx)),
        );
    }

    pub fn notify_stream_wait(key: &str, info: Option<BaseStreamInfo>) -> bool {
        let guard = GENERAL_CACHE.shared.state.lock();
        let sender = guard
            .entities
            .get(key)
            .and_then(|entry| match &entry.waiter {
                Some(StateWaiter::Stream(tx)) => Some(tx.clone()),
                _ => None,
            });
        drop(guard);
        match sender {
            Some(tx) => {
                let _ = tx.try_send(info);
                true
            }
            None => false,
        }
    }

    pub fn insert_snapshot_wait(key: String, expire: Instant, tx: Sender<bool>) {
        Self::upsert_state(
            key,
            StateValue::Empty,
            Some(expire),
            Some(StateWaiter::Snapshot(tx)),
        );
    }

    pub fn notify_snapshot_wait(key: &str) -> bool {
        let guard = GENERAL_CACHE.shared.state.lock();
        let sender = guard
            .entities
            .get(key)
            .and_then(|entry| match &entry.waiter {
                Some(StateWaiter::Snapshot(tx)) => Some(tx.clone()),
                _ => None,
            });
        drop(guard);
        match sender {
            Some(tx) => {
                let _ = tx.try_send(true);
                true
            }
            None => false,
        }
    }

    pub fn insert_counter(key: String, count: u8, expire: Duration) {
        Self::upsert_state(
            key,
            StateValue::Counter(count),
            Some(Instant::now() + expire),
            None,
        );
    }

    pub fn remove_state(key: &str) -> bool {
        let mut guard = GENERAL_CACHE.shared.state.lock();
        let removed = guard.entities.remove(key);
        drop(guard);
        let _ = Register::scheduler().remove_general_cache(key);
        removed.is_some()
    }

    pub fn refresh_state(key: &str, expire: Duration) -> bool {
        let mut guard = GENERAL_CACHE.shared.state.lock();
        let refreshed = match guard.entities.get_mut(key) {
            Some(entry) => {
                entry.expire = Some(Instant::now() + expire);
                true
            }
            None => false,
        };
        drop(guard);
        if refreshed {
            let _ = Register::scheduler().insert_general_cache(key.to_string(), expire);
        }
        refreshed
    }

    pub fn decrement_counter(key: String) -> bool {
        let mut guard = GENERAL_CACHE.shared.state.lock();
        let handled = guard.decrement_counter(key.clone());
        drop(guard);
        if handled == Some(true) {
            let _ = Register::scheduler().remove_general_cache(&key);
        }
        handled.is_some()
    }

    fn init_ssrc_sn() -> DashSet<u16> {
        let sets = DashSet::new();
        for i in 1..10000 {
            sets.insert(i);
        }
        sets
    }

    fn init() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    entities: HashMap::new(),
                }),
                ssrc_sn: Self::init_ssrc_sn(),
                stream_map: Default::default(),
                device_map: Default::default(),
                stream_setup_locks: Default::default(),
            }),
        }
    }

    pub fn purge_expired_keys(keys: Vec<String>) {
        GENERAL_CACHE.shared.purge_expired_keys(keys);
    }
}

struct State {
    entities: HashMap<String, StateEntry>,
}

impl State {
    fn decrement_counter(&mut self, key: String) -> Option<bool> {
        match self.entities.entry(key) {
            Occupied(mut occ) => {
                let entry = occ.get_mut();
                let should_remove = match &mut entry.value {
                    StateValue::Counter(val) => {
                        if *val <= 1 {
                            true
                        } else {
                            *val -= 1;
                            false
                        }
                    }
                    _ => true,
                };
                if should_remove {
                    let _ = occ.remove_entry();
                }
                Some(should_remove)
            }
            Vacant(_) => None,
        }
    }
}

enum StateValue {
    Empty,
    Counter(u8),
}

enum StateWaiter {
    Stream(Sender<Option<BaseStreamInfo>>),
    Snapshot(Sender<bool>),
}

struct StateEntry {
    value: StateValue,
    expire: Option<Instant>,
    waiter: Option<StateWaiter>,
}

struct StreamTable {
    gmv_token_sets: HashSet<String>,
    proxy_addr: String,
    stream_node_name: String,
    call_id: String,
    seq: u32,
    am: AccessMode,
    from_tag: String,
    to_tag: String,
    ssrc: u32,
}

struct DeviceTable {
    channel_id: String,
    am: AccessMode,
    stream_id: String,
    config: MediaConfig,
    ssrc: String,
}

struct Shared {
    state: Mutex<State>,
    ssrc_sn: DashSet<u16>,
    stream_map: DashMap<String, StreamTable>,
    device_map: DashMap<String, Vec<DeviceTable>>,
    stream_setup_locks: DashMap<String, Arc<AsyncMutex<()>>>,
}

impl Shared {
    fn purge_expired_keys(&self, keys: Vec<String>) {
        let mut state = self.state.lock();
        let now = Instant::now();
        for key in keys {
            let expired = match state.entities.get(&key).and_then(|entry| entry.expire) {
                Some(expire) => expire <= now,
                None => false,
            };
            if !expired {
                continue;
            }
            if let Some(entry) = state.entities.remove(&key) {
                let _ = entry.expire;
                if let Some(waiter) = entry.waiter {
                    match waiter {
                        StateWaiter::Stream(tx) => {
                            let _ = tx.try_send(None);
                        }
                        StateWaiter::Snapshot(tx) => {
                            let _ = tx.try_send(false);
                        }
                    }
                }
            }
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Hash)]
pub enum AccessMode {
    Live,
    Back,
    Down,
}

impl AccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AccessMode::Live => "live",
            AccessMode::Back => "back",
            AccessMode::Down => "down",
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::state::session::{AccessMode, Cache, GENERAL_CACHE, StreamTable};
    use base::dashmap::{DashMap, DashSet};
    use base::rand;
    use base::rand::prelude::IteratorRandom;

    #[test]
    fn test_ref_mut() {
        let table = StreamTable {
            gmv_token_sets: Default::default(),
            proxy_addr: "".to_string(),
            stream_node_name: "".to_string(),
            call_id: "".to_string(),
            seq: 0,
            am: AccessMode::Live,
            from_tag: "".to_string(),
            to_tag: "".to_string(),
            ssrc: 0,
        };
        let map = DashMap::new();
        map.insert(1, table);
        map.get_mut(&1).map(|mut ref_mut| {
            let stream_table = ref_mut.value_mut();
            stream_table.seq += 1;
        });
        println!("{:?}", map.get_mut(&1).map(|item| item.value().seq));
    }

    #[test]
    fn test_ssrc_sn() {
        let ssrc_sn = Cache::ssrc_sn_get().unwrap();
        println!("ssrc_sn = {ssrc_sn}");
        assert_eq!(GENERAL_CACHE.shared.ssrc_sn.len(), 9998);
        assert!(!GENERAL_CACHE.shared.ssrc_sn.contains(&ssrc_sn));
        Cache::ssrc_sn_set(ssrc_sn);
        assert_eq!(GENERAL_CACHE.shared.ssrc_sn.len(), 9999);
    }

    #[test]
    fn test_rand_remove() {
        let sets = DashSet::new();
        for i in 1..10000 {
            sets.insert(i);
        }

        let mut rng = rand::thread_rng();
        for _i in 0..10 {
            if let Some(val) = sets.iter().choose(&mut rng).map(|v| *v) {
                match sets.remove(&val) {
                    None => {
                        println!("end");
                    }
                    Some(val) => {
                        println!("val = {}", val);
                        sets.insert(val);
                    }
                }
            };
        }
    }
}
