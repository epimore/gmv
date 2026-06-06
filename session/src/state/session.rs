use std::collections::hash_map::Entry::{Occupied, Vacant};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
static STREAM_CLOSE_GENERATION: AtomicU64 = AtomicU64::new(1);

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

#[derive(Clone)]
pub struct StreamByeCommand {
    pub stream_id: String,
    pub generation: u64,
    pub device_id: String,
    pub channel_id: String,
    pub ssrc: u32,
    pub call_id: String,
    pub seq: u32,
    pub remote_target: String,
    pub route_set: Vec<String>,
    pub from_header: String,
    pub to_header: String,
}

pub struct StreamCloseStart {
    pub generation: u64,
    pub device_id: String,
    pub newly_started: bool,
}

pub struct StreamCloseInfo {
    pub stream_id: String,
    pub generation: u64,
    pub device_id: String,
    pub channel_id: String,
    pub ssrc: u32,
    pub call_id: String,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct TalkSessionState {
    pub talk_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub ssrc: u32,
    pub stream_node_name: String,
    pub call_id: String,
    pub seq: u32,
    pub from_tag: String,
    pub to_tag: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channel_count: u8,
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
        device_id: String,
        channel_id: String,
        ssrc: u32,
        proxy_addr: String,
        stream_node_name: String,
        call_id: String,
        seq: u32,
        am: AccessMode,
        from_tag: String,
        to_tag: String,
        remote_target: String,
        route_set: Vec<String>,
        from_header: String,
        to_header: String,
    ) -> bool {
        match GENERAL_CACHE.shared.stream_map.entry(stream_id) {
            Entry::Occupied(_) => false,
            Entry::Vacant(vac) => {
                vac.insert(StreamTable {
                    gmv_token_sets: HashSet::new(),
                    device_id,
                    channel_id,
                    proxy_addr,
                    stream_node_name,
                    call_id,
                    seq,
                    am,
                    from_tag,
                    to_tag,
                    remote_target,
                    route_set,
                    from_header,
                    to_header,
                    ssrc,
                    lifecycle: StreamLifecycle::Playing,
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

    pub fn stream_is_closing(stream_id: &str) -> bool {
        GENERAL_CACHE
            .shared
            .stream_map
            .get(stream_id)
            .is_some_and(|stream| stream.is_closing())
    }

    pub fn stream_close_begin(stream_id: &str) -> Option<StreamCloseStart> {
        let mut stream = GENERAL_CACHE.shared.stream_map.get_mut(stream_id)?;
        let generation = STREAM_CLOSE_GENERATION.fetch_add(1, Ordering::Relaxed);
        let newly_started = stream.begin_close(generation);
        Some(StreamCloseStart {
            generation: stream.closing_generation()?,
            device_id: stream.device_id.clone(),
            newly_started,
        })
    }

    pub fn stream_close_take_bye(stream_id: &str) -> Option<StreamByeCommand> {
        let mut stream = GENERAL_CACHE.shared.stream_map.get_mut(stream_id)?;
        let (generation, seq) = stream.take_bye()?;
        Some(StreamByeCommand {
            stream_id: stream_id.to_string(),
            generation,
            device_id: stream.device_id.clone(),
            channel_id: stream.channel_id.clone(),
            ssrc: stream.ssrc,
            call_id: stream.call_id.clone(),
            seq,
            remote_target: stream.remote_target.clone(),
            route_set: stream.route_set.clone(),
            from_header: stream.from_header.clone(),
            to_header: stream.to_header.clone(),
        })
    }

    pub fn stream_close_mark_failed(
        stream_id: &str,
        generation: u64,
        seq: u32,
        reason: String,
    ) -> bool {
        GENERAL_CACHE
            .shared
            .stream_map
            .get_mut(stream_id)
            .is_some_and(|mut stream| stream.mark_bye_failed(generation, seq, reason))
    }

    pub fn stream_close_ids_by_device(device_id: &str) -> Vec<String> {
        GENERAL_CACHE
            .shared
            .device_map
            .get(device_id)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        Self::stream_is_closing(&entry.stream_id)
                            .then(|| entry.stream_id.clone())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn stream_close_complete(
        stream_id: &str,
        generation: u64,
    ) -> Option<StreamCloseInfo> {
        Self::stream_close_remove(stream_id, generation)
    }

    pub fn stream_close_force(stream_id: &str, generation: u64) -> Option<StreamCloseInfo> {
        Self::stream_close_remove(stream_id, generation)
    }

    fn stream_close_remove(stream_id: &str, generation: u64) -> Option<StreamCloseInfo> {
        let (_, stream) = GENERAL_CACHE
            .shared
            .stream_map
            .remove_if(stream_id, |_, stream| {
                stream.is_closing_generation(generation)
            })?;
        Self::device_map_remove_stream(&stream.device_id, stream_id);
        GENERAL_CACHE
            .shared
            .ssrc_sn
            .insert((stream.ssrc % 10000) as u16);
        let _ = Register::scheduler().remove_register(
            &crate::register::core::TimeScheduleKey::StreamClosing(
                Arc::from(stream_id),
                generation,
            ),
        );
        let last_error = stream.last_error();
        Some(StreamCloseInfo {
            stream_id: stream_id.to_string(),
            generation,
            device_id: stream.device_id,
            channel_id: stream.channel_id,
            ssrc: stream.ssrc,
            call_id: stream.call_id,
            last_error,
        })
    }

    fn device_map_remove_stream(device_id: &str, stream_id: &str) {
        match GENERAL_CACHE.shared.device_map.entry(device_id.to_string()) {
            Entry::Occupied(mut entry) => {
                entry
                    .get_mut()
                    .retain(|device_stream| device_stream.stream_id != stream_id);
                if entry.get().is_empty() {
                    entry.remove();
                }
            }
            Entry::Vacant(_) => {}
        }
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

    pub fn talk_map_insert(state: TalkSessionState) -> bool {
        match GENERAL_CACHE.shared.talk_map.entry(state.talk_id.clone()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(vac) => {
                vac.insert(state);
                true
            }
        }
    }

    pub fn talk_map_remove(talk_id: &str) -> Option<TalkSessionState> {
        GENERAL_CACHE
            .shared
            .talk_map
            .remove(talk_id)
            .map(|(_, state)| state)
    }

    pub fn talk_map_get_by_device_channel(
        device_id: &str,
        channel_id: &str,
    ) -> Option<TalkSessionState> {
        GENERAL_CACHE.shared.talk_map.iter().find_map(|item| {
            let value = item.value();
            if value.device_id == device_id && value.channel_id == channel_id {
                Some(value.clone())
            } else {
                None
            }
        })
    }

    pub fn reset_device_state(device_id: &str) -> Vec<DeviceStreamState> {
        let mut streams = Vec::new();
        if let Some((_, entries)) = GENERAL_CACHE.shared.device_map.remove(device_id) {
            for entry in entries {
                if let Some((_, stream)) = GENERAL_CACHE.shared.stream_map.remove(&entry.stream_id)
                {
                    if let Some(generation) = stream.closing_generation() {
                        let _ = Register::scheduler().remove_register(
                            &crate::register::core::TimeScheduleKey::StreamClosing(
                                Arc::from(entry.stream_id.as_str()),
                                generation,
                            ),
                        );
                    }
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
        let talk_ids = GENERAL_CACHE
            .shared
            .talk_map
            .iter()
            .filter_map(|item| {
                if item.device_id == device_id {
                    Some(item.talk_id.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for talk_id in talk_ids {
            if let Some((_, talk)) = GENERAL_CACHE.shared.talk_map.remove(&talk_id) {
                let ssrc_num = (talk.ssrc % 10000) as u16;
                GENERAL_CACHE.shared.ssrc_sn.insert(ssrc_num);
                streams.push(DeviceStreamState {
                    channel_id: talk.channel_id,
                    stream_id: talk.talk_id,
                    ssrc: talk.ssrc.to_string(),
                    call_id: talk.call_id,
                    seq: talk.seq.saturating_add(1),
                    from_tag: talk.from_tag,
                    to_tag: talk.to_tag,
                });
            }
        }
        let setup_lock_prefix = format!("{device_id}:");
        GENERAL_CACHE
            .shared
            .stream_setup_locks
            .retain(|key, _| !key.starts_with(&setup_lock_prefix));
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
                talk_map: Default::default(),
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
    device_id: String,
    channel_id: String,
    proxy_addr: String,
    stream_node_name: String,
    call_id: String,
    seq: u32,
    am: AccessMode,
    from_tag: String,
    to_tag: String,
    remote_target: String,
    route_set: Vec<String>,
    from_header: String,
    to_header: String,
    ssrc: u32,
    lifecycle: StreamLifecycle,
}

enum StreamLifecycle {
    Playing,
    Closing {
        generation: u64,
        inflight_seq: Option<u32>,
        last_error: Option<String>,
    },
}

impl StreamTable {
    fn begin_close(&mut self, generation: u64) -> bool {
        if matches!(self.lifecycle, StreamLifecycle::Closing { .. }) {
            return false;
        }
        self.lifecycle = StreamLifecycle::Closing {
            generation,
            inflight_seq: None,
            last_error: None,
        };
        true
    }

    fn take_bye(&mut self) -> Option<(u64, u32)> {
        let StreamLifecycle::Closing {
            generation,
            inflight_seq,
            ..
        } = &mut self.lifecycle
        else {
            return None;
        };
        if inflight_seq.is_some() {
            return None;
        }
        self.seq = self.seq.saturating_add(1);
        *inflight_seq = Some(self.seq);
        Some((*generation, self.seq))
    }

    fn mark_bye_failed(
        &mut self,
        expected_generation: u64,
        expected_seq: u32,
        reason: String,
    ) -> bool {
        let StreamLifecycle::Closing {
            generation,
            inflight_seq,
            last_error,
        } = &mut self.lifecycle
        else {
            return false;
        };
        if *generation != expected_generation || *inflight_seq != Some(expected_seq) {
            return false;
        }
        *inflight_seq = None;
        *last_error = Some(reason);
        true
    }

    fn is_closing(&self) -> bool {
        matches!(self.lifecycle, StreamLifecycle::Closing { .. })
    }

    fn is_closing_generation(&self, expected_generation: u64) -> bool {
        matches!(
            self.lifecycle,
            StreamLifecycle::Closing { generation, .. }
                if generation == expected_generation
        )
    }

    fn closing_generation(&self) -> Option<u64> {
        match self.lifecycle {
            StreamLifecycle::Playing => None,
            StreamLifecycle::Closing { generation, .. } => Some(generation),
        }
    }

    fn last_error(&self) -> Option<String> {
        match &self.lifecycle {
            StreamLifecycle::Playing => None,
            StreamLifecycle::Closing { last_error, .. } => last_error.clone(),
        }
    }
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
    talk_map: DashMap<String, TalkSessionState>,
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
    Talk,
}

impl AccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AccessMode::Live => "live",
            AccessMode::Back => "back",
            AccessMode::Down => "down",
            AccessMode::Talk => "talk",
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::state::session::{
        AccessMode, Cache, GENERAL_CACHE, StreamLifecycle, StreamTable,
    };
    use base::dashmap::{DashMap, DashSet};
    use base::rand;
    use base::rand::prelude::IteratorRandom;

    fn stream_table() -> StreamTable {
        StreamTable {
            gmv_token_sets: Default::default(),
            device_id: "device-id".to_string(),
            channel_id: "channel-id".to_string(),
            proxy_addr: "".to_string(),
            stream_node_name: "".to_string(),
            call_id: "call-id".to_string(),
            seq: 7,
            am: AccessMode::Live,
            from_tag: "from-tag".to_string(),
            to_tag: "to-tag".to_string(),
            remote_target: "sip:device@127.0.0.1:5060".to_string(),
            route_set: Vec::new(),
            from_header: "<sip:platform@127.0.0.1>;tag=from-tag".to_string(),
            to_header: "<sip:device@127.0.0.1>;tag=to-tag".to_string(),
            ssrc: 1001,
            lifecycle: StreamLifecycle::Playing,
        }
    }

    #[test]
    fn test_ref_mut() {
        let table = stream_table();
        let map = DashMap::new();
        map.insert(1, table);
        map.get_mut(&1).map(|mut ref_mut| {
            let stream_table = ref_mut.value_mut();
            stream_table.seq += 1;
        });
        println!("{:?}", map.get_mut(&1).map(|item| item.value().seq));
    }

    #[test]
    fn closing_stream_allows_only_one_bye_in_flight() {
        let mut table = stream_table();

        assert!(table.begin_close(11));
        assert_eq!(table.take_bye(), Some((11, 8)));
        assert_eq!(table.take_bye(), None);
    }

    #[test]
    fn failed_bye_can_retry_with_next_cseq() {
        let mut table = stream_table();

        assert!(table.begin_close(11));
        assert_eq!(table.take_bye(), Some((11, 8)));
        assert!(table.mark_bye_failed(11, 8, "tcp closed".to_string()));
        assert_eq!(table.take_bye(), Some((11, 9)));
    }

    #[test]
    fn stale_bye_failure_does_not_clear_new_inflight_bye() {
        let mut table = stream_table();

        assert!(table.begin_close(11));
        assert_eq!(table.take_bye(), Some((11, 8)));
        assert!(table.mark_bye_failed(11, 8, "old tcp closed".to_string()));
        assert_eq!(table.take_bye(), Some((11, 9)));
        assert!(!table.mark_bye_failed(11, 8, "old tcp closed".to_string()));
        assert_eq!(table.take_bye(), None);
    }

    #[test]
    fn repeated_close_keeps_original_generation() {
        let mut table = stream_table();

        assert!(table.begin_close(11));
        assert!(!table.begin_close(12));
        assert!(table.is_closing_generation(11));
        assert!(!table.is_closing_generation(12));
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

    #[test]
    fn reset_device_state_removes_only_matching_setup_locks() {
        let device_id = "34020000001320009991";
        let other_device_id = "34020000001320009992";
        Cache::stream_setup_lock(device_id, "channel-1", AccessMode::Live);
        Cache::stream_setup_lock(device_id, "channel-2", AccessMode::Talk);
        Cache::stream_setup_lock(other_device_id, "channel-1", AccessMode::Live);

        Cache::reset_device_state(device_id);

        let prefix = format!("{device_id}:");
        assert!(
            GENERAL_CACHE
                .shared
                .stream_setup_locks
                .iter()
                .all(|entry| !entry.key().starts_with(&prefix))
        );
        assert!(
            GENERAL_CACHE
                .shared
                .stream_setup_locks
                .contains_key(&format!("{other_device_id}:channel-1:live"))
        );
        GENERAL_CACHE
            .shared
            .stream_setup_locks
            .remove(&format!("{other_device_id}:channel-1:live"));
    }
}
