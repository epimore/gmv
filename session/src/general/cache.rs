use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::collections::hash_map::Entry::{Occupied, Vacant};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use bimap::BiMap;
use log::{error, warn};
use mysql::serde_json;
use serde::{Deserialize, Serialize};

use common::bytes::Bytes;
use common::dashmap::DashMap;
use common::dashmap::mapref::entry::Entry;
use common::err::TransError;
use common::once_cell::sync::Lazy;
use common::tokio;
use common::tokio::sync::{Mutex, Notify};
use common::tokio::sync::mpsc::Sender;
use common::tokio::time;
use common::tokio::time::Instant;

static GENERAL_CACHE: Lazy<Cache> = Lazy::new(|| Cache::init());


pub struct Cache {
    shared: Arc<Shared>,
}

impl Cache {
    //添加流与用户关系：
    //当不存在流时:直接插入stream_id与新建的set<gmv_token>
    //当存在流时:在流对应的set<gmv_token>中添加数据
    pub fn stream_map_insert(stream_id: String, gmv_token: String) {
        match GENERAL_CACHE.shared.stream_map.entry(stream_id) {
            Entry::Occupied(mut occ) => {
                occ.get_mut().insert(gmv_token);
            }
            Entry::Vacant(vac) => {
                let mut set = HashSet::new();
                set.insert(gmv_token);
                vac.insert(set);
            }
        }
    }

    //移除流与用户关系
    //1.当gmv_token为None时-直接删除
    //2.当gmv_token为Some时-删除set<gmv_token>中的gmv_token：如果set<gmv_token>中只有一条该gmv_token,则如第1项
    pub fn stream_map_remove(stream_id: &String, gmv_token: Option<&String>) {
        match gmv_token {
            None => { GENERAL_CACHE.shared.stream_map.remove(stream_id); }
            Some(token) => {
                match GENERAL_CACHE.shared.stream_map.entry(stream_id.to_string()) {
                    Entry::Occupied(mut occ) => {
                        match occ.get().len() {
                            0 => {
                                occ.remove();
                            }
                            1 => {
                                if occ.get().contains(token) {
                                    occ.remove();
                                }
                            }
                            _ => {
                                occ.get_mut().remove(token);
                            }
                        }
                    }
                    Entry::Vacant(vac) => {}
                }
            }
        }
    }

    //确认流与用户是否建立了关系
    pub fn stream_map_contains_token(stream_id: &String, gmv_token: &String) -> bool {
        GENERAL_CACHE.shared.stream_map
            .get(stream_id)
            .map(|sets| sets.contains(gmv_token))
            .unwrap_or(false)
    }
    //device_id:HashMap<channel_id,HashMap<playType,Vec<(stream_id,ssrc)>>
    //层层插入
    pub fn device_map_insert(device_id: String, channel_id: String, ssrc: String, stream_id: String, play_type: PlayType) -> bool {
        match GENERAL_CACHE.shared.device_map.entry(device_id) {
            Entry::Occupied(mut occ) => {
                let m_map = occ.get_mut();
                match m_map.entry(channel_id) {
                    Occupied(mut m_occ) => {
                        let s_map = m_occ.get_mut();
                        match s_map.entry(play_type) {
                            Occupied(mut s_occ) => {
                                //直播只插入一条
                                if play_type == PlayType::Live {
                                    false
                                } else {
                                    let s_map = s_occ.get_mut();
                                    s_map.insert_no_overwrite(stream_id, ssrc).is_ok()
                                }
                            }
                            //存在device_id,channel_id,不存在ssrc则:在channel_id对应的map中插入
                            Vacant(s_vac) => {
                                let mut bi_map = BiMap::new();
                                bi_map.insert(stream_id, ssrc);
                                s_vac.insert(bi_map);
                                true
                            }
                        }
                    }
                    //存在device_id,不存在channel_id则：在存在device_id对应的map中插入channel_id与ssrc对应的map
                    Vacant(m_vac) => {
                        let mut bi_map = BiMap::new();
                        bi_map.insert(stream_id, ssrc);
                        let mut s_map = HashMap::new();
                        s_map.insert(play_type, bi_map);
                        m_vac.insert(s_map);
                        true
                    }
                }
            }
            //不存在device_id则全新插入
            Entry::Vacant(vac) => {
                let mut m_map = HashMap::new();
                let mut s_map = HashMap::new();
                let mut bi_map = BiMap::new();
                bi_map.insert(stream_id, ssrc);
                s_map.insert(play_type, bi_map);
                m_map.insert(channel_id, s_map);
                vac.insert(m_map);
                true
            }
        }
    }

    //层层删除：若最终device_id对应的都无数据，则整体删除
    //device_id: String, channel_id: String, ssrc: String
    pub fn device_map_remove(device_id: &String, opt_channel_ssrc: Option<(&String, Option<(PlayType, &String)>)>) {
        match opt_channel_ssrc {
            None => {
                GENERAL_CACHE.shared.device_map.remove(device_id);
            }
            Some((channel_id, None)) => {
                match GENERAL_CACHE.shared.device_map.entry(device_id.to_string()) {
                    // 如果其值包含channel_id,且仅一条数据，则删除device_id
                    // 否则删除channel_id
                    Entry::Occupied(mut m_occ) => {
                        let s_map = m_occ.get_mut();
                        if s_map.contains_key(channel_id) {
                            if s_map.len() == 1 {
                                m_occ.remove();
                            } else {
                                s_map.remove(channel_id);
                            }
                        }
                    }
                    //与device_id不匹配，不做处理
                    Entry::Vacant(m_vac) => {}
                }
            }
            //存在SSRC:删除里层SSRC,若删除时，device_id对应的map将无数据,则直接删除device_id
            Some((channel_id, Some((play_type, ssrc)))) => {
                match GENERAL_CACHE.shared.device_map.entry(device_id.to_string()) {
                    // 如果其值包含channel_id,ssrc,且仅一条数据，则删除device_id
                    // 如果其值包含channel_id,有多条ssrc，则删除ssrc记录
                    // 如果其值包含多条channel_id,一条ssrc，则删除ssrc对应的channel_id
                    Entry::Occupied(mut m_occ) => {
                        let m_map = m_occ.get_mut();
                        let m_len = m_map.len();
                        match m_map.entry(channel_id.to_string()) {
                            Occupied(mut s_occ) => {
                                let s_map = s_occ.get_mut();
                                let s_len = s_map.len();
                                match s_map.entry(play_type) {
                                    Occupied(mut i_occ) => {
                                        let i_map = i_occ.get_mut();
                                        let i_len = i_map.len();
                                        if i_map.contains_right(ssrc) {
                                            if i_len == 1 {
                                                if s_len == 1 {
                                                    if m_len == 1 {
                                                        m_occ.remove();
                                                    } else {
                                                        s_occ.remove();
                                                    }
                                                } else {
                                                    i_occ.remove();
                                                }
                                            } else {
                                                i_map.remove_by_right(ssrc);
                                            }
                                        }
                                    }
                                    Vacant(i_vac) => {}
                                }
                            }
                            Vacant(s_vac) => {}
                        }
                    }
                    //与device_id不匹配，不做处理
                    Entry::Vacant(m_vac) => {}
                }
            }
        }
    }

    pub fn device_map_count(device_id: &String, opt_channel_ssrc: Option<(&String, Option<(&PlayType, Option<&String>)>)>) -> usize {
        match opt_channel_ssrc {
            None => {
                GENERAL_CACHE.shared.device_map.get(device_id).map(|m_map| m_map.len()).unwrap_or(0)
            }
            Some((channel_id, None)) => {
                GENERAL_CACHE.shared.device_map.get(device_id)
                    .map(|m_map| m_map.get(channel_id)
                        .map(|s_map| s_map.len()).unwrap_or(0))
                    .unwrap_or(0)
            }
            Some((channel_id, Some((play_type, None)))) => {
                GENERAL_CACHE.shared.device_map.get(device_id)
                    .map(|m_map| m_map.get(channel_id)
                        .map(|s_map| if s_map.contains_key(play_type) { 1 } else { 0 }).unwrap_or(0))
                    .unwrap_or(0)
            }
            Some((channel_id, Some((play_type, Some(ssrc))))) => {
                GENERAL_CACHE.shared.device_map.get(device_id)
                    .map(|m_map| m_map.get(channel_id)
                        .map(|s_map| s_map.get(play_type)
                            .map(|i_map| if i_map.contains_right(ssrc) { 1 } else { 0 }).unwrap_or(0))
                        .unwrap_or(0))
                    .unwrap_or(0)
            }
        }
    }

    async fn state_insert(key: String, data: Bytes, expire: Option<Instant>, call_tx: Option<Sender<Option<Bytes>>>) {
        let mut guard = GENERAL_CACHE.shared.state.lock().await;
        match expire {
            None => { guard.entities.insert(key, (data, None, call_tx)); }
            Some(ins) => {
                guard.entities.insert(key.clone(), (data, Some(ins), call_tx));
                guard.expirations.insert((ins, key));
            }
        }
    }

    pub async fn state_insert_obj<'a, T: Serialize + Deserialize<'a>>(key: String, obj: &T, call_tx: Option<Sender<Option<Bytes>>>) {
        //此处不会panic，obj满足序列化与反序列化
        let vec = serde_json::to_vec(obj).unwrap();
        let bytes = Bytes::from(vec);
        Self::state_insert(key, bytes, None, call_tx).await;
    }
    pub async fn state_insert_obj_by_timer<'a, T: Serialize + Deserialize<'a>>(key: String, obj: &T, expire: Duration, call_tx: Option<Sender<Option<Bytes>>>) {
        //此处不会panic，obj满足序列化与反序列化
        let vec = serde_json::to_vec(obj).unwrap();
        let bytes = Bytes::from(vec);
        let when = Instant::now() + expire;
        Self::state_insert(key, bytes, Some(when), call_tx).await;
    }

    pub

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
                    stream_map: Default::default(),
                    device_map: Default::default(),
                }
            )
        };
        let shared = cache.shared.clone();
        thread::Builder::new().name("General:Cache".to_string()).spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().hand_err(|msg| error!("{msg}")).unwrap();
            let _ = rt.block_on(Self::purge_expired_task(shared));
        }).expect("General:Cache background thread create failed");
        cache
    }

    async fn purge_expired_task(shared: Arc<Shared>) {
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

struct State {
    //key,(obj,是否启用时间轮,是否回调:超时回调None)
    entities: HashMap<String, (Bytes, Option<Instant>, Option<Sender<Option<Bytes>>>)>,
    expirations: BTreeSet<(Instant, String)>,
}

impl State {
    fn next_expiration(&self) -> Option<Instant> {
        self.expirations.first().map(|expiration| expiration.0)
    }
}

struct Shared {
    state: Mutex<State>,
    background_task: Notify,
    //stream_id:set<gmv_token>
    stream_map: DashMap<String, HashSet<String>>,
    //device_id:HashMap<channel_id,HashMap<ssrc,(stream_id,playType)>>
    // device_map: DashMap<String, HashMap<String, HashMap<String, (String, PlayType)>>>,
    //device_id:HashMap<channel_id,HashMap<playType,BiMap<stream_id,ssrc>>
    device_map: DashMap<String, HashMap<String, HashMap<PlayType, BiMap<String, String>>>>,
}

impl Shared {
    async fn purge_expired_keys(&self) -> Option<Instant> {
        let now = Instant::now();
        let mut state = self.state.lock().await;
        let state = &mut *state;
        while let Some((when, key)) = state.expirations.iter().next() {
            if *when > now {
                return Some(*when);
            }
            if let Some((_, _, Some(tx))) = state.entities.remove(key) {
                let _ = tx.send(None).await.hand_err(|msg| warn!("{msg}"));
            }
            state.expirations.remove(&(*when, key.clone()));
        }
        None
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Hash)]
pub enum PlayType {
    Live,
    Back,
    Down,
}

impl PlayType {}

#[cfg(test)]
mod tests {
    use crate::general::cache::{Cache, GENERAL_CACHE, PlayType};

    #[test]
    fn test_stream_map() {
        Cache::stream_map_insert("ID1".to_string(), "TOKEN1".to_string());
        Cache::stream_map_insert("ID2".to_string(), "TOKEN2".to_string());
        Cache::stream_map_insert("ID1".to_string(), "XXX".to_string());

        Cache::stream_map_insert("ID1".to_string(), "ABAB".to_string());
        Cache::stream_map_insert("ID3".to_string(), "xx3".to_string());
        Cache::stream_map_insert("ID4".to_string(), "xx4".to_string());

        Cache::stream_map_remove(&"ID4".to_string(), None);
        Cache::stream_map_remove(&"ID1".to_string(), Some(&"ABAB".to_string()));
        Cache::stream_map_remove(&"ID2".to_string(), Some(&"TMP".to_string()));
        Cache::stream_map_remove(&"ID3".to_string(), Some(&"xx3".to_string()));

        let mut size = 0;
        for en in GENERAL_CACHE.shared.stream_map.iter() {
            let key = en.key();
            println!("{key}");
            let sets = en.value();
            let mut iter = sets.iter();
            if key[..].eq("ID1") {
                assert_eq!(sets.len(), 2);
                assert!(sets.contains("TOKEN1"));
                assert!(sets.contains("XXX"));
                println!("{:?}", iter.next());
                println!("{:?}", iter.next());
            }
            if key[..].eq("ID2") {
                assert_eq!(sets.len(), 1);
                assert!(sets.contains("TOKEN2"));
                println!("{:?}", iter.next());
            }
            size += 1;
        }
        assert_eq!(GENERAL_CACHE.shared.stream_map.len(), 2);
        assert!(Cache::stream_map_contains_token(&"ID1".to_string(), &"TOKEN1".to_string()), "not contains {} : {}", "ID1", "TOKEN1");
        assert!(!Cache::stream_map_contains_token(&"ID2".to_string(), &"TOKEN1".to_string()));
        assert!(!Cache::stream_map_contains_token(&"ID3".to_string(), &"xx3".to_string()));
    }

    #[test]
    fn test_device_map() {
        Cache::device_map_insert("did1".to_string(), "cid1".to_string(), "ssrc1".to_string(), "sid1".to_string(), PlayType::Down);
        let len = GENERAL_CACHE.shared.device_map.len();
        assert_eq!(len, 1);
        let len1 = Cache::device_map_count(&"did1".to_string(), Some((&"cid1".to_string(), Some((&PlayType::Down, Some(&"ssrc1".to_string()))))));
        assert_eq!(len1, 1);
        Cache::device_map_remove(&"did1".to_string(), Some((&"cid1".to_string(), Some((PlayType::Down, &"ssrc1".to_string())))));
        let len2 = GENERAL_CACHE.shared.device_map.len();
        assert_eq!(len2, 0);
    }
}