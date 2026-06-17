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
static TALK_CLOSE_GENERATION: AtomicU64 = AtomicU64::new(1);
static CATALOG_SUBSCRIPTION_GENERATION: AtomicU64 = AtomicU64::new(1);

pub struct Cache {
    shared: Arc<Shared>,
}

#[derive(Clone)]
pub struct StreamByeCommand {
    pub stream_id: String,
    pub generation: u64,
    pub device_id: String,
    pub call_id: String,
    pub seq: u32,
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
pub struct CatalogSubscriptionCommand {
    pub generation: u64,
    pub call_id: String,
    pub seq: u32,
    pub event: String,
    pub expires: u32,
    pub remote_target: String,
    pub route_set: Vec<String>,
    pub from_header: String,
    pub to_header: String,
}

struct CatalogSubscriptionState {
    generation: u64,
    call_id: String,
    seq: u32,
    event: String,
    expires: u32,
    remote_target: String,
    route_set: Vec<String>,
    from_header: String,
    to_header: String,
    local_tag: String,
    remote_tag: String,
    inflight: bool,
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
    pub closing_generation: Option<u64>,
    pub bye_inflight_seq: Option<u32>,
    pub close_last_error: Option<String>,
}

pub struct TalkCloseStart {
    pub generation: u64,
    pub device_id: String,
    pub newly_started: bool,
}

pub struct TalkByeCommand {
    pub talk_id: String,
    pub generation: u64,
    pub device_id: String,
    pub call_id: String,
    pub seq: u32,
}

pub struct TalkCloseInfo {
    pub talk_id: String,
    pub generation: u64,
    pub device_id: String,
    pub channel_id: String,
    pub ssrc: u32,
    pub call_id: String,
    pub last_error: Option<String>,
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
                    ssrc,
                    lifecycle: StreamLifecycle::Playing,
                });
                true
            }
        }
    }

    pub fn stream_device_id(stream_id: &str) -> Option<String> {
        GENERAL_CACHE
            .shared
            .stream_map
            .get(stream_id)
            .map(|stream| stream.device_id.clone())
    }

    pub fn stream_call_id(stream_id: &str) -> Option<String> {
        GENERAL_CACHE
            .shared
            .stream_map
            .get(stream_id)
            .map(|stream| stream.call_id.clone())
    }

    pub fn stream_map_query_node_ssrc(stream_id: &String) -> Option<(String, u32)> {
        GENERAL_CACHE.shared.stream_map.get(stream_id).map(|item| {
            let node_name = item.stream_node_name.clone();
            (node_name, item.ssrc)
        })
    }

    pub fn stream_map_update_source(
        stream_id: &str,
        proxy_addr: String,
        stream_node_name: String,
    ) -> bool {
        GENERAL_CACHE
            .shared
            .stream_map
            .get_mut(stream_id)
            .is_some_and(|mut stream| {
                stream.proxy_addr = proxy_addr;
                stream.stream_node_name = stream_node_name;
                true
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
            call_id: stream.call_id.clone(),
            seq,
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
            .stream_map
            .iter()
            .filter_map(|stream| {
                (stream.device_id == device_id && stream.is_closing()).then(|| stream.key().clone())
            })
            .collect()
    }

    pub fn stream_close_complete(stream_id: &str, generation: u64) -> Option<StreamCloseInfo> {
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
        Self::finish_removed_stream(stream_id, stream, generation)
    }

    pub fn stream_terminated_by_call_id(call_id: &str) -> Option<StreamCloseInfo> {
        let stream_id = GENERAL_CACHE
            .shared
            .stream_map
            .iter()
            .find_map(|stream| (stream.call_id == call_id).then(|| stream.key().clone()))?;
        let (_, stream) = GENERAL_CACHE
            .shared
            .stream_map
            .remove_if(&stream_id, |_, stream| stream.call_id == call_id)?;
        let generation = stream.closing_generation().unwrap_or_default();
        Self::finish_removed_stream(&stream_id, stream, generation)
    }

    pub fn stream_ids_for_media_status(device_id: &str, channel_id: &str) -> Vec<String> {
        GENERAL_CACHE
            .shared
            .device_map
            .get(device_id)
            .map(|streams| {
                streams
                    .iter()
                    .filter(|stream| {
                        stream.channel_id == channel_id
                            && matches!(stream.am, AccessMode::Back | AccessMode::Down)
                    })
                    .map(|stream| stream.stream_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn finish_removed_stream(
        stream_id: &str,
        stream: StreamTable,
        generation: u64,
    ) -> Option<StreamCloseInfo> {
        Self::device_map_remove_stream(&stream.device_id, stream_id);
        GENERAL_CACHE
            .shared
            .ssrc_sn
            .insert((stream.ssrc % 10000) as u16);
        if let Some(closing_generation) = stream.closing_generation() {
            let _ = Register::scheduler().remove_register(
                &crate::register::core::TimeScheduleKey::StreamClosing(
                    Arc::from(stream_id),
                    closing_generation,
                ),
            );
        }
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
        let talk = GENERAL_CACHE
            .shared
            .talk_map
            .remove(talk_id)
            .map(|(_, state)| state)?;
        Self::cancel_talk_close_timer(talk_id, &talk);
        Some(talk)
    }

    pub fn talk_map_get(talk_id: &str) -> Option<TalkSessionState> {
        GENERAL_CACHE
            .shared
            .talk_map
            .get(talk_id)
            .map(|talk| talk.clone())
    }

    pub fn talk_map_remove_by_call_id(call_id: &str) -> Option<TalkSessionState> {
        let talk_id = GENERAL_CACHE
            .shared
            .talk_map
            .iter()
            .find_map(|talk| (talk.call_id == call_id).then(|| talk.key().clone()))?;
        let talk = GENERAL_CACHE
            .shared
            .talk_map
            .remove_if(&talk_id, |_, talk| talk.call_id == call_id)
            .map(|(_, talk)| talk)?;
        Self::cancel_talk_close_timer(&talk_id, &talk);
        Some(talk)
    }

    fn cancel_talk_close_timer(talk_id: &str, talk: &TalkSessionState) {
        let Some(generation) = talk.closing_generation else {
            return;
        };
        if let Some(scheduler) = crate::register::schedule::TimeScheduler::try_global() {
            let _ =
                scheduler.remove_register(&crate::register::core::TimeScheduleKey::TalkClosing(
                    Arc::from(talk_id),
                    generation,
                ));
        }
    }

    pub fn talk_close_begin(talk_id: &str) -> Option<TalkCloseStart> {
        let mut talk = GENERAL_CACHE.shared.talk_map.get_mut(talk_id)?;
        let newly_started = talk.closing_generation.is_none();
        if newly_started {
            talk.closing_generation = Some(TALK_CLOSE_GENERATION.fetch_add(1, Ordering::Relaxed));
            talk.bye_inflight_seq = None;
            talk.close_last_error = None;
        }
        Some(TalkCloseStart {
            generation: talk.closing_generation?,
            device_id: talk.device_id.clone(),
            newly_started,
        })
    }

    pub fn talk_close_take_bye(talk_id: &str) -> Option<TalkByeCommand> {
        let mut talk = GENERAL_CACHE.shared.talk_map.get_mut(talk_id)?;
        let generation = talk.closing_generation?;
        if talk.bye_inflight_seq.is_some() {
            return None;
        }
        talk.seq = talk.seq.saturating_add(1);
        talk.bye_inflight_seq = Some(talk.seq);
        Some(TalkByeCommand {
            talk_id: talk_id.to_string(),
            generation,
            device_id: talk.device_id.clone(),
            call_id: talk.call_id.clone(),
            seq: talk.seq,
        })
    }

    pub fn talk_close_mark_failed(
        talk_id: &str,
        generation: u64,
        seq: u32,
        reason: String,
    ) -> bool {
        GENERAL_CACHE
            .shared
            .talk_map
            .get_mut(talk_id)
            .is_some_and(|mut talk| {
                if talk.closing_generation != Some(generation) || talk.bye_inflight_seq != Some(seq)
                {
                    return false;
                }
                talk.bye_inflight_seq = None;
                talk.close_last_error = Some(reason);
                true
            })
    }

    pub fn talk_close_ids_by_device(device_id: &str) -> Vec<String> {
        GENERAL_CACHE
            .shared
            .talk_map
            .iter()
            .filter_map(|talk| {
                (talk.device_id == device_id && talk.closing_generation.is_some())
                    .then(|| talk.talk_id.clone())
            })
            .collect()
    }

    pub fn talk_close_complete(talk_id: &str, generation: u64) -> Option<TalkCloseInfo> {
        let (_, talk) = GENERAL_CACHE
            .shared
            .talk_map
            .remove_if(talk_id, |_, talk| {
                talk.closing_generation == Some(generation)
            })?;
        GENERAL_CACHE
            .shared
            .ssrc_sn
            .insert((talk.ssrc % 10000) as u16);
        if let Some(scheduler) = crate::register::schedule::TimeScheduler::try_global() {
            let _ =
                scheduler.remove_register(&crate::register::core::TimeScheduleKey::TalkClosing(
                    Arc::from(talk_id),
                    generation,
                ));
        }
        Some(TalkCloseInfo {
            talk_id: talk_id.to_string(),
            generation,
            device_id: talk.device_id,
            channel_id: talk.channel_id,
            ssrc: talk.ssrc,
            call_id: talk.call_id,
            last_error: talk.close_last_error,
        })
    }

    pub fn talk_close_force(talk_id: &str, generation: u64) -> Option<TalkCloseInfo> {
        Self::talk_close_complete(talk_id, generation)
    }

    pub fn has_dialog_call_id(call_id: &str) -> bool {
        GENERAL_CACHE
            .shared
            .stream_map
            .iter()
            .any(|stream| stream.call_id == call_id)
            || GENERAL_CACHE
                .shared
                .talk_map
                .iter()
                .any(|talk| talk.call_id == call_id)
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

    #[allow(clippy::too_many_arguments)]
    pub fn catalog_subscription_begin(
        device_id: String,
        call_id: String,
        seq: u32,
        event: String,
        expires: u32,
        remote_target: String,
        from_header: String,
        to_header: String,
        local_tag: String,
    ) -> Option<u64> {
        match GENERAL_CACHE.shared.catalog_subscriptions.entry(device_id) {
            Entry::Occupied(_) => None,
            Entry::Vacant(entry) => {
                let generation = CATALOG_SUBSCRIPTION_GENERATION.fetch_add(1, Ordering::Relaxed);
                entry.insert(CatalogSubscriptionState {
                    generation,
                    call_id,
                    seq,
                    event,
                    expires,
                    remote_target,
                    route_set: Vec::new(),
                    from_header,
                    to_header,
                    local_tag,
                    remote_tag: String::new(),
                    inflight: true,
                });
                Some(generation)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn catalog_subscription_complete(
        device_id: &str,
        generation: u64,
        remote_target: String,
        route_set: Vec<String>,
        from_header: String,
        to_header: String,
        remote_tag: String,
    ) -> bool {
        GENERAL_CACHE
            .shared
            .catalog_subscriptions
            .get_mut(device_id)
            .is_some_and(|mut subscription| {
                if subscription.generation != generation {
                    return false;
                }
                subscription.remote_target = remote_target;
                subscription.route_set = route_set;
                subscription.from_header = from_header;
                subscription.to_header = to_header;
                subscription.remote_tag = remote_tag;
                subscription.inflight = false;
                true
            })
    }

    pub fn catalog_subscription_take_refresh(
        device_id: &str,
        generation: u64,
    ) -> Option<CatalogSubscriptionCommand> {
        let mut subscription = GENERAL_CACHE
            .shared
            .catalog_subscriptions
            .get_mut(device_id)?;
        if subscription.generation != generation || subscription.inflight {
            return None;
        }
        subscription.seq = subscription.seq.saturating_add(1);
        subscription.inflight = true;
        Some(CatalogSubscriptionCommand {
            generation,
            call_id: subscription.call_id.clone(),
            seq: subscription.seq,
            event: subscription.event.clone(),
            expires: subscription.expires,
            remote_target: subscription.remote_target.clone(),
            route_set: subscription.route_set.clone(),
            from_header: subscription.from_header.clone(),
            to_header: subscription.to_header.clone(),
        })
    }

    pub fn catalog_subscription_mark_failed(device_id: &str, generation: u64) -> bool {
        GENERAL_CACHE
            .shared
            .catalog_subscriptions
            .get_mut(device_id)
            .is_some_and(|mut subscription| {
                if subscription.generation != generation {
                    return false;
                }
                subscription.inflight = false;
                true
            })
    }

    pub fn catalog_subscription_update_expires(
        device_id: &str,
        generation: u64,
        expires: u32,
    ) -> bool {
        GENERAL_CACHE
            .shared
            .catalog_subscriptions
            .get_mut(device_id)
            .is_some_and(|mut subscription| {
                if subscription.generation != generation {
                    return false;
                }
                subscription.expires = expires;
                true
            })
    }

    pub fn catalog_subscription_expires(device_id: &str, generation: u64) -> Option<u32> {
        GENERAL_CACHE
            .shared
            .catalog_subscriptions
            .get(device_id)
            .and_then(|subscription| {
                (subscription.generation == generation).then_some(subscription.expires)
            })
    }

    pub fn catalog_subscription_validate_notify(
        device_id: &str,
        call_id: &str,
        event: &str,
        remote_tag: Option<&str>,
        local_tag: Option<&str>,
    ) -> Option<u64> {
        let mut subscription = GENERAL_CACHE
            .shared
            .catalog_subscriptions
            .get_mut(device_id)?;
        if subscription.call_id != call_id
            || !catalog_event_matches(&subscription.event, event)
            || local_tag != Some(subscription.local_tag.as_str())
        {
            return None;
        }
        match remote_tag {
            Some(tag) if subscription.remote_tag.is_empty() => {
                subscription.remote_tag = tag.to_string();
            }
            Some(tag) if tag == subscription.remote_tag => {}
            _ => return None,
        }
        Some(subscription.generation)
    }

    pub fn catalog_subscription_remove(device_id: &str, generation: Option<u64>) -> bool {
        let removed = match generation {
            Some(generation) => GENERAL_CACHE
                .shared
                .catalog_subscriptions
                .remove_if(device_id, |_, state| state.generation == generation),
            None => GENERAL_CACHE.shared.catalog_subscriptions.remove(device_id),
        };
        if let Some((_, subscription)) = &removed {
            if let Some(scheduler) = crate::register::schedule::TimeScheduler::try_global() {
                let _ = scheduler.remove_register(
                    &crate::register::core::TimeScheduleKey::CatalogSubscription(
                        Arc::from(device_id),
                        subscription.generation,
                    ),
                );
            }
        }
        removed.is_some()
    }

    pub fn reset_device_state(device_id: &str) {
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
                }
            }
        }
        let remaining_stream_ids = GENERAL_CACHE
            .shared
            .stream_map
            .iter()
            .filter_map(|stream| (stream.device_id == device_id).then(|| stream.key().clone()))
            .collect::<Vec<_>>();
        for stream_id in remaining_stream_ids {
            if let Some((_, stream)) = GENERAL_CACHE.shared.stream_map.remove(&stream_id) {
                if let Some(generation) = stream.closing_generation() {
                    if let Some(scheduler) = crate::register::schedule::TimeScheduler::try_global()
                    {
                        let _ = scheduler.remove_register(
                            &crate::register::core::TimeScheduleKey::StreamClosing(
                                Arc::from(stream_id.as_str()),
                                generation,
                            ),
                        );
                    }
                }
                GENERAL_CACHE
                    .shared
                    .ssrc_sn
                    .insert((stream.ssrc % 10000) as u16);
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
                if let Some(generation) = talk.closing_generation {
                    if let Some(scheduler) = crate::register::schedule::TimeScheduler::try_global()
                    {
                        let _ = scheduler.remove_register(
                            &crate::register::core::TimeScheduleKey::TalkClosing(
                                Arc::from(talk_id.as_str()),
                                generation,
                            ),
                        );
                    }
                }
                let ssrc_num = (talk.ssrc % 10000) as u16;
                GENERAL_CACHE.shared.ssrc_sn.insert(ssrc_num);
            }
        }
        let setup_lock_prefix = format!("{device_id}:");
        GENERAL_CACHE
            .shared
            .stream_setup_locks
            .retain(|key, _| !key.starts_with(&setup_lock_prefix));
        Self::catalog_subscription_remove(device_id, None);
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
                catalog_subscriptions: Default::default(),
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

fn catalog_event_matches(expected: &str, actual: &str) -> bool {
    fn parts(value: &str) -> (&str, Option<&str>) {
        let mut parts = value.split(';').map(str::trim);
        let package = parts.next().unwrap_or_default();
        let id = parts.find_map(|part| {
            let (key, value) = part.split_once('=')?;
            key.trim()
                .eq_ignore_ascii_case("id")
                .then_some(value.trim())
        });
        (package, id)
    }

    let (expected_package, expected_id) = parts(expected);
    let (actual_package, actual_id) = parts(actual);
    expected_package.eq_ignore_ascii_case(actual_package)
        && match actual_id {
            Some(actual_id) => expected_id == Some(actual_id),
            None => true,
        }
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
    catalog_subscriptions: DashMap<String, CatalogSubscriptionState>,
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
    use crate::state::session::{AccessMode, Cache, GENERAL_CACHE, StreamLifecycle, StreamTable};

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
            ssrc: 1001,
            lifecycle: StreamLifecycle::Playing,
        }
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
        assert_eq!(GENERAL_CACHE.shared.ssrc_sn.len(), 9998);
        assert!(!GENERAL_CACHE.shared.ssrc_sn.contains(&ssrc_sn));
        Cache::ssrc_sn_set(ssrc_sn);
        assert_eq!(GENERAL_CACHE.shared.ssrc_sn.len(), 9999);
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

    #[test]
    fn peer_terminated_dialog_removes_stream_and_releases_ssrc() {
        let stream_id = "peer-bye-stream".to_string();
        let ssrc = 8765u32;
        GENERAL_CACHE.shared.ssrc_sn.remove(&8765);
        Cache::stream_map_insert_info(
            stream_id.clone(),
            "peer-bye-device".to_string(),
            "peer-bye-channel".to_string(),
            ssrc,
            String::new(),
            String::new(),
            "peer-bye-call-id".to_string(),
            1,
            AccessMode::Live,
        );

        let removed = Cache::stream_terminated_by_call_id("peer-bye-call-id").unwrap();

        assert_eq!(removed.stream_id, stream_id);
        assert!(Cache::stream_map_query_node_ssrc(&removed.stream_id).is_none());
        assert!(GENERAL_CACHE.shared.ssrc_sn.contains(&8765));
    }

    #[test]
    fn catalog_subscription_is_singleton_and_refreshes_cseq() {
        let device_id = "catalog-device";
        Cache::catalog_subscription_remove(device_id, None);
        let generation = Cache::catalog_subscription_begin(
            device_id.to_string(),
            "catalog-call-id".to_string(),
            20,
            "Catalog;id=123".to_string(),
            3600,
            "sip:device@192.0.2.10:5060".to_string(),
            "<sip:platform@example.com>;tag=local-tag".to_string(),
            "<sip:device@example.com>".to_string(),
            "local-tag".to_string(),
        )
        .unwrap();

        assert!(
            Cache::catalog_subscription_begin(
                device_id.to_string(),
                "other-call-id".to_string(),
                1,
                "Catalog;id=456".to_string(),
                3600,
                "sip:device@192.0.2.10:5060".to_string(),
                "<sip:platform@example.com>;tag=other".to_string(),
                "<sip:device@example.com>".to_string(),
                "other".to_string(),
            )
            .is_none()
        );

        Cache::catalog_subscription_complete(
            device_id,
            generation,
            "sip:device@192.0.2.10:5080".to_string(),
            vec!["<sip:proxy.example.com;lr>".to_string()],
            "<sip:platform@example.com>;tag=local-tag".to_string(),
            "<sip:device@example.com>;tag=remote-tag".to_string(),
            "remote-tag".to_string(),
        );
        let command = Cache::catalog_subscription_take_refresh(device_id, generation).unwrap();

        assert_eq!(command.seq, 21);
        assert_eq!(command.call_id, "catalog-call-id");
        assert_eq!(command.remote_target, "sip:device@192.0.2.10:5080");
        Cache::catalog_subscription_remove(device_id, Some(generation));
    }

    #[test]
    fn catalog_notify_requires_matching_dialog() {
        let device_id = "catalog-notify-device";
        Cache::catalog_subscription_remove(device_id, None);
        let generation = Cache::catalog_subscription_begin(
            device_id.to_string(),
            "notify-call-id".to_string(),
            30,
            "Catalog;id=789".to_string(),
            3600,
            "sip:device@192.0.2.20:5060".to_string(),
            "<sip:platform@example.com>;tag=local-tag".to_string(),
            "<sip:device@example.com>".to_string(),
            "local-tag".to_string(),
        )
        .unwrap();
        Cache::catalog_subscription_complete(
            device_id,
            generation,
            "sip:device@192.0.2.20:5060".to_string(),
            Vec::new(),
            "<sip:platform@example.com>;tag=local-tag".to_string(),
            "<sip:device@example.com>;tag=remote-tag".to_string(),
            "remote-tag".to_string(),
        );

        assert_eq!(
            Cache::catalog_subscription_validate_notify(
                device_id,
                "notify-call-id",
                "Catalog;id=789",
                Some("remote-tag"),
                Some("local-tag"),
            ),
            Some(generation)
        );
        assert!(
            Cache::catalog_subscription_validate_notify(
                device_id,
                "other-call-id",
                "Catalog;id=789",
                Some("remote-tag"),
                Some("local-tag"),
            )
            .is_none()
        );
        assert!(
            Cache::catalog_subscription_validate_notify(
                device_id,
                "notify-call-id",
                "Catalog;id=999",
                Some("remote-tag"),
                Some("local-tag"),
            )
            .is_none()
        );
        Cache::catalog_subscription_remove(device_id, Some(generation));
    }

    #[test]
    fn talk_close_keeps_dialog_until_terminal_response() {
        let talk_id = "closing-talk".to_string();
        Cache::talk_map_remove(&talk_id);
        Cache::talk_map_insert(super::TalkSessionState {
            talk_id: talk_id.clone(),
            device_id: "talk-device".to_string(),
            channel_id: "talk-channel".to_string(),
            ssrc: 4567,
            stream_node_name: "s1".to_string(),
            call_id: "talk-call-id".to_string(),
            seq: 8,
            closing_generation: None,
            bye_inflight_seq: None,
            close_last_error: None,
        });

        let start = Cache::talk_close_begin(&talk_id).unwrap();
        let command = Cache::talk_close_take_bye(&talk_id).unwrap();

        assert!(Cache::talk_map_get(&talk_id).is_some());
        assert_eq!(command.seq, 9);
        assert!(Cache::talk_close_complete(&talk_id, start.generation).is_some());
        assert!(Cache::talk_map_get(&talk_id).is_none());
    }
}
