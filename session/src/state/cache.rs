use std::collections::hash_map::Entry::{Occupied, Vacant};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::state;
use base::bytes::Bytes;
use base::dashmap::mapref::entry::Entry;
use base::dashmap::{DashMap, DashSet};
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::once_cell::sync::Lazy;
use base::rand::seq::IteratorRandom;
use base::serde::de::DeserializeOwned;
use base::serde::{Deserialize, Serialize};
use base::tokio::sync::mpsc::Sender;
use base::tokio::sync::Notify;
use base::tokio::time;
use base::tokio::time::Instant;
use base::{rand, serde_json, tokio};
use base::tokio_util::sync::CancellationToken;
use base::utils::rt::GlobalRuntime;
use shared::info::media_info::MediaConfig;

static GENERAL_CACHE: Lazy<Cache> = Lazy::new(|| Cache::init());

pub struct Cache {
    shared: Arc<Shared>,
}

impl Cache {
    pub fn ssrc_sn_get() -> Option<u16> {
        let mut rng = rand::thread_rng();
        if let Some(val) = GENERAL_CACHE.shared.ssrc_sn.iter().choose(&mut rng).map(|v| *v) {
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
                    let count = occ.get_mut();
                    *count += 1;
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

    //添加流与用户关系：
    //当不存在流时:直接插入stream_id与新建的set<gmv_token>
    //当存在流时:在流对应的set<gmv_token>中添加数据
    pub fn stream_map_insert_token(stream_id: String, gmv_token: String) -> bool {
        match GENERAL_CACHE.shared.stream_map.entry(stream_id) {
            Entry::Occupied(mut occ) => {
                let stream_table = occ.get_mut();
                let opt_sets = &mut stream_table.gmv_token_sets;
                opt_sets.insert(gmv_token);
                true
            }
            Entry::Vacant(_vac) => {
                false
            }
        }
    }

    //当媒体流注册时，需插入建立关系,成功插入：true
    pub fn stream_map_insert_info(stream_id: String, stream_node_name: String, call_id: String, seq: u32, am: AccessMode, from_tag: String, to_tag: String) -> bool {
        match GENERAL_CACHE.shared.stream_map.entry(stream_id) {
            Entry::Occupied(_) => { false }
            Entry::Vacant(vac) => {
                let stream_table = StreamTable {
                    gmv_token_sets: HashSet::new(),
                    stream_node_name,
                    call_id,
                    seq,
                    am,
                    from_tag,
                    to_tag,
                };
                vac.insert(stream_table);
                true
            }
        }
    }

    pub fn stream_map_query_node_name(stream_id: &String) -> Option<String> {
        GENERAL_CACHE.shared.stream_map.get(stream_id)
            .map(|item| {
                let node_name = item.value().stream_node_name.clone();
                node_name
            })
    }

    //移除流与用户关系
    //1.当gmv_token为None时-直接删除
    //2.当gmv_token为Some时-删除set<gmv_token>中的gmv_token：
    pub fn stream_map_remove(stream_id: &String, gmv_token: Option<&String>) {
        match gmv_token {
            None => {
                GENERAL_CACHE.shared.stream_map.remove(stream_id);
            }
            Some(token) => {
                match GENERAL_CACHE.shared.stream_map.entry(stream_id.to_string()) {
                    Entry::Occupied(mut occ) => {
                        let sets = &mut occ.get_mut().gmv_token_sets;
                        sets.remove(token);
                    }
                    Entry::Vacant(_vac) => {}
                }
            }
        }
    }


    //确认流与用户是否建立了关系
    pub fn stream_map_contains_token(stream_id: &String, gmv_token: &String) -> bool {
        match GENERAL_CACHE.shared.stream_map.get(stream_id) {
            None => { false }
            Some(inner_ref) => {
                let sets = &inner_ref.value().gmv_token_sets;
                sets.contains(gmv_token)
            }
        }
    }

    pub fn stream_map_build_call_id_seq_from_to_tag(stream_id: &String) -> Option<(String, u32, String, String)> {
        GENERAL_CACHE.shared.stream_map.get_mut(stream_id)
            .map(|mut ref_mut| {
                // let (_tokens, _node_name, call_id, seq, _play_type, from_tag, to_tag) = ref_mut.value_mut();
                let stream_table = ref_mut.value_mut();
                let seq = &mut stream_table.seq;
                *seq += 1;
                (stream_table.call_id.clone(), *seq, stream_table.from_tag.clone(), stream_table.to_tag.clone())
            })
    }

    pub fn stream_map_query_play_type_by_stream_id(stream_id: &String) -> Option<AccessMode> {
        GENERAL_CACHE.shared.stream_map.get(stream_id).map(|res| {
            let am = res.value().am;
            am.clone()
        })
    }

    //device_id:HashMap<channel_id,HashMap<playType,Vec<(stream_id,ssrc)>>
    //层层插入
    pub fn device_map_insert(device_id: String, channel_id: String, ssrc: String, stream_id: String, am: AccessMode, config: MediaConfig) {
        let device_table = DeviceTable {
            channel_id,
            am,
            stream_id,
            config,
            ssrc,
        };
        match GENERAL_CACHE.shared.device_map.entry(device_id) {
            Entry::Occupied(mut occ) => {
                let vec = occ.get_mut();
                vec.push(device_table);
            }
            //不存在device_id则全新插入
            Entry::Vacant(vac) => {
                let vec = vec![device_table];
                vac.insert(vec);
            }
        }
    }

    //层层删除：若最终device_id对应的都无数据，则整体删除
    //device_id: String, channel_id: String, ssrc: String
    /*
    1.opt_channel_ssrc = none => remove(device_id)
    2.opt_channel_ssrc = some
      2.1 (PlayType,ssrc)= none => remove(device_id下channel_id)
      2.2 (PlayType,ssrc)= some => remove(device_id下channel_id下(PlayType,ssrc))
    */
    pub fn device_map_remove(device_id: &String, opt_channel_ssrc: Option<(&String, Option<(AccessMode, &String)>)>) {
        match opt_channel_ssrc {
            None => {
                GENERAL_CACHE.shared.device_map.remove(device_id);
            }
            Some((channel_id, channel_ssrc)) => {
                match GENERAL_CACHE.shared.device_map.entry(device_id.to_string()) {
                    Entry::Occupied(mut m_occ) => {
                        let s_vec = m_occ.get_mut();
                        s_vec.retain(|device_table| {
                            match channel_ssrc {
                                None => {
                                    !device_table.channel_id.eq(channel_id)
                                }
                                Some((am, ssrc)) => {
                                    !device_table.channel_id.eq(channel_id)
                                        && !device_table.am.eq(&am)
                                        && !device_table.ssrc.eq(ssrc)
                                }
                            }
                        });
                        // 如果vec empty，则删除device_id
                        if s_vec.len() == 0 {
                            m_occ.remove();
                        }
                    }
                    //与device_id不匹配，不做处理
                    Entry::Vacant(_m_vac) => {}
                }
            }
        }
    }

    //返回stream_id,ssrc
    pub fn device_map_get_invite_info(device_id: &String, channel_id: &String, am: &AccessMode) -> Option<(String, String)> {
        match GENERAL_CACHE.shared.device_map.get(device_id) {
            None => { None }
            Some(m_map) => {
                let mut iter = m_map.value().iter();
                iter.find_map(|device_table| {
                    if device_table.channel_id.eq(channel_id) && device_table.am.eq(am) {
                        return Some((device_table.stream_id.clone(), device_table.ssrc.clone()));
                    }
                    None
                }
                )
            }
        }
    }

    pub fn state_insert(key: String, data: Bytes, expire: Option<Instant>, call_tx: Option<Sender<Option<Bytes>>>) {
        let mut guard = GENERAL_CACHE.shared.state.lock();

        match expire {
            None => { guard.entities.insert(key, (data, None, call_tx)); }
            Some(ins) => {
                guard.entities.insert(key.clone(), (data, Some(ins), call_tx));
                guard.expirations.insert((ins, key));
            }
        }
    }

    pub fn state_get(key: &str) -> Option<(Bytes, Option<Sender<Option<Bytes>>>)> {
        let guard = GENERAL_CACHE.shared.state.lock();

        match guard.entities.get(key) {
            None => {
                None
            }
            Some((val, _, opt_tx)) => {
                Some((val.clone(), opt_tx.clone()))
            }
        }
    }

    pub fn state_insert_obj<'a, T: Serialize + Deserialize<'a>>(key: String, obj: &T, call_tx: Option<Sender<Option<Bytes>>>) {
        //此处不会panic，obj满足序列化与反序列化
        let vec = serde_json::to_vec(obj).unwrap();
        let bytes = Bytes::from(vec);
        Self::state_insert(key, bytes, None, call_tx);
    }

    pub fn state_insert_obj_by_timer<'a, T: Serialize + Deserialize<'a>>(key: String, obj: &T, expire: Duration, call_tx: Option<Sender<Option<Bytes>>>) {
        //此处不会panic，obj满足序列化与反序列化
        let vec = serde_json::to_vec(obj).unwrap();
        let bytes = Bytes::from(vec);
        let when = Instant::now() + expire;
        Self::state_insert(key, bytes, Some(when), call_tx);
    }

    pub fn state_remove(key: &String) -> Option<(Bytes, Option<Sender<Option<Bytes>>>)> {
        let mut guard = GENERAL_CACHE.shared.state.lock();

        if let Some((data, Some(when), opt_tx)) = guard.entities.remove(key) {
            guard.expirations.remove(&(when, key.to_string()));

            return Some((data, opt_tx));
        }

        None
    }

    pub fn state_get_obj<T: Serialize + DeserializeOwned>(key: &str) -> GlobalResult<Option<(T, Option<Sender<Option<Bytes>>>)>> {
        let guard = GENERAL_CACHE.shared.state.lock();

        match guard.entities.get(key) {
            None => {
                Ok(None)
            }
            Some((val, _, opt_tx)) => {
                let data: T = serde_json::from_slice(&val.clone()).hand_log(|msg| error!("{msg}"))?;

                Ok(Some((data, opt_tx.clone())))
            }
        }
    }

    fn init_ssrc_sn() -> DashSet<u16> {
        let sets = DashSet::new();
        for i in 1..10000 {
            sets.insert(i);
        }
        sets
    }

    fn init() -> Self {
        let cache = Self {
            shared: Arc::new(
                Shared {
                    state: Mutex::new(
                        State {
                            entities: HashMap::new(),
                            expirations: BTreeSet::new(),
                        }
                    ),
                    background_task: Notify::new(),
                    ssrc_sn: Self::init_ssrc_sn(),
                    stream_map: Default::default(),
                    device_map: Default::default(),
                }
            )
        };
        let shared = cache.shared.clone();
        let rt = GlobalRuntime::get_main_runtime();
        rt.rt_handle.spawn(Self::purge_expired_task(shared,rt.cancel));
        cache
    }

    async fn purge_expired_task(shared: Arc<Shared>,cancel_token: CancellationToken,) {
        loop {
            if cancel_token.is_cancelled() { break; }
            if let Some(when) = shared.purge_expired_keys().await {
                tokio::select! {
                    _ = time::sleep_until(when) =>{},
                    _ = shared.background_task.notified()=>{}
                }
            } else {
                shared.background_task.notified().await;
            }
        }
        let mut state = shared.state.lock();
        state.expirations.clear();
        state.entities.clear();
    }
}

struct State {
    //key,(obj,是否启用时间轮,是否回调:超时回调None)
    entities: HashMap<String, (Bytes, Option<Instant>, Option<Sender<Option<Bytes>>>)>,
    expirations: BTreeSet<(Instant, String)>,
}

#[allow(unused)]
impl State {
    fn next_expiration(&self) -> Option<Instant> {
        self.expirations.first().map(|expiration| expiration.0)
    }
}

struct StreamTable {
    gmv_token_sets: HashSet<String>,
    stream_node_name: String,
    call_id: String,
    seq: u32,
    am: AccessMode,
    from_tag: String,
    to_tag: String,
}

struct DeviceTable {
    channel_id: String,
    am: AccessMode,
    stream_id: String,
    config: MediaConfig,
    ssrc: String,
}

//下个大版本-抽象会话
//voice  发言
//dialog  对话
//context  上下文
//session  会话
struct Shared {
    state: Mutex<State>,
    background_task: Notify,
    //存放原始可用的ssrc序号
    ssrc_sn: DashSet<u16>,
    //key:stream_id
    stream_map: DashMap<String, StreamTable>,
    //key:device_id
    device_map: DashMap<String, Vec<DeviceTable>>,
}

impl Shared {
    async fn purge_expired_keys(&self) -> Option<Instant> {
        let now = Instant::now();

        let mut state = self.state.lock();

        let state = &mut *state;
        while let Some((when, key)) = state.expirations.iter().next() {
            if *when > now {
                return Some(*when);
            }
            if let Some((_, _, Some(tx))) = state.entities.remove(key) {
                let _ = tx.try_send(None).hand_log(|msg| warn!("{msg}"));
            }
            state.expirations.remove(&(*when, key.clone()));
        }

        None
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Hash)]
pub enum AccessMode {
    Live,
    Back,
    Down,
}

impl AccessMode {}

#[cfg(test)]
mod tests {
    use crate::state::cache::{AccessMode, Cache, StreamTable, GENERAL_CACHE};
    use base::dashmap::{DashMap, DashSet};
    use base::rand;
    use base::rand::prelude::IteratorRandom;

    #[test]
    fn test_ref_mut() {
        let table = StreamTable {
            gmv_token_sets: Default::default(),
            stream_node_name: "".to_string(),
            call_id: "".to_string(),
            seq: 0,
            am: AccessMode::Live,
            from_tag: "".to_string(),
            to_tag: "".to_string(),
        };
        let map = DashMap::new();
        map.insert(1, table);
        map.get_mut(&1)
            .map(|mut ref_mut| {
                let stream_table = ref_mut.value_mut();
                let seq = &mut stream_table.seq;
                *seq += 1;
            });
        println!("{:?}", map.get_mut(&1).map(|item| item.value().seq));
    }

    #[test]
    fn test_ssrc_sn() {
        let ssrc_sn = Cache::ssrc_sn_get().unwrap();
        println!("ssrc_sn = {ssrc_sn}");
        assert_eq!(GENERAL_CACHE.shared.ssrc_sn.len(), 9998);
        assert_eq!(GENERAL_CACHE.shared.ssrc_sn.contains(&ssrc_sn), false);
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
                    None => { println!("end"); }
                    Some(val) => {
                        println!("val = {}", val);
                        sets.insert(val);
                    }
                }
            };
        }
    }
}