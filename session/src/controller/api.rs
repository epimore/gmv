
use poem::FromRequest;
use poem_openapi::OpenApi;
use poem_openapi::payload::{Form, Json};
use crate::general::model::{PlayLiveModel, ResultMessageData, StreamInfo};

pub struct RestApi;

#[OpenApi(prefix_path = "/api")]
impl RestApi {
    #[allow(non_snake_case)]
    #[oai(path = "/play/live/stream", method = "post")]
    /// 点播监控实时画面 transMode 默认0 udp 模式, 1 tcp 被动模式,2 tcp 主动模式， 目前只支持 0
    async fn play_live(&self, live: Json<PlayLiveModel>) -> Json<ResultMessageData<Option<StreamInfo>>> {
        println!("{:?}",live.0);
        Json(ResultMessageData::build_success_none())
    }

    //
    // #[allow(non_snake_case)]
    // #[oai(path = "/play/save/info", method = "get")]
    // /// 查看录像信息
    // async fn down_info(&self,
    //                    #[oai(name = "deviceId", validator(min_length = "20", max_length = "20"))] deviceId: Query<String>,
    //                    #[oai(name = "channelId", validator(min_length = "20", max_length = "20"))] channelId: Query<String>) -> Json<ResultMessageData<Option<Vec<RecordInfo>>>> {
    //     match handler::query_down_info(&deviceId.0, &channelId.0).await {
    //         Err(err) => {
    //             error!("查看录像信息失败；{}",err);
    //             Json(ResultMessageData::build_failure())
    //         }
    //         Ok(info) => { Json(ResultMessageData::build_success(Some(info))) }
    //     }
    // }
    //
    // #[allow(non_snake_case)]
    // #[oai(path = "/play/back/save", method = "get")]
    // /// 开启监控历史画面云端录制 transMode 默认0 udp 模式, 1 tcp 被动模式,2 tcp 主动模式， 目前只支持 0 下载速度124   同人同录像机同摄像头只能同时下载一路监控
    // async fn download(&self,
    //                   #[oai(name = "deviceId", validator(min_length = "20", max_length = "20"))] deviceId: Query<String>,
    //                   #[oai(name = "channelId", validator(min_length = "20", max_length = "20"))] channelId: Query<String>,
    //                   #[oai(name = "identity", validator(min_length = "4", max_length = "32"))] _identity: Query<String>,
    //                   #[oai(name = "fileName")] _fileName: Query<String>,
    //                   #[oai(name = "st", validator(minimum(value = "1577808000")))] st: Query<u32>,
    //                   #[oai(name = "et", validator(minimum(value = "1577808001")))] et: Query<u32>,
    //                   #[oai(name = "speed", validator(maximum(value = "4"), minimum(value = "1")))] _speed: Query<u8>,
    //                   #[oai(name = "transMode", validator(maximum(value = "2"), minimum(value = "0")))] _transMode: Query<u8>) -> Json<ResultMessageData<Option<bool>>> {
    //     let dt = Local::now();
    //     match handler::down(&deviceId.0, &channelId.0, 0, st.0, et.0, 4, "twoLevel", dt.timestamp().to_string()).await {
    //         Err(err) => {
    //             error!("下载失败；{}",err);
    //             Json(ResultMessageData::build_failure())
    //         }
    //         Ok(b) => { Json(ResultMessageData::build_success(Some(b))) }
    //     }
    // }
    //
    // #[allow(non_snake_case)]
    // #[oai(path = "/play/save/break", method = "get")]
    // /// 提前终止云端录像任务
    // async fn save_break(&self,
    //                     #[oai(name = "id", validator(min_length = "32", max_length = "32"))] id: Query<String>) -> Json<ResultMessageData<Option<bool>>> {
    //     match handler::teardown_save(&id.0).await {
    //         Err(err) => {
    //             error!("终止失败；{}",err);
    //             Json(ResultMessageData::build_failure())
    //         }
    //         Ok(_info) => { Json(ResultMessageData::build_success_none()) }
    //     }
    // }
    //
    // #[allow(non_snake_case)]
    // #[oai(path = "/play/back/stream", method = "get")]
    // /// 点播监控历史画面 transMode 默认0 udp 模式, 1 tcp 被动模式,2 tcp 主动模式， 目前只支持 0  时间跨度不超过24H
    // /// 相机不能同时观看不同的时间段监控？？？是否优化？不同人观看不同？前端设备可以抗住几路并发
    // async fn playback(&self,
    //                   #[oai(name = "deviceId", validator(min_length = "20", max_length = "20"))] deviceId: Query<String>,
    //                   #[oai(name = "channelId", validator(min_length = "20", max_length = "20"))] channelId: Query<String>,
    //                   // #[oai(name = "userId", validator(min_length = "4", max_length = "32"))] _userId: Query<String>,
    //                   #[oai(name = "st", validator(minimum(value = "1577808000")))] st: Query<u32>,
    //                   #[oai(name = "et", validator(minimum(value = "1577808001")))] et: Query<u32>,
    //                   #[oai(name = "transMode", validator(maximum(value = "2"), minimum(value = "0")))] _transMode: Query<u8>) -> Json<ResultMessageData<Option<StreamInfo>>> {
    //     match handler::playback(&deviceId.0, &channelId.0, 0, st.0, et.0, "twoLevel").await {
    //         Err(err) => {
    //             error!("点播失败；{}",err);
    //             Json(ResultMessageData::build_failure())
    //         }
    //         Ok(info) => { Json(ResultMessageData::build_success(Some(info))) }
    //     }
    // }
    // #[allow(non_snake_case)]
    // #[oai(path = "/play/back/seek", method = "get")]
    // /// 拖动播放录像 seek 拖动秒 [1-86400]
    // async fn playback_seek(&self,
    //                        #[oai(name = "streamId", validator(min_length = "32", max_length = "32"))] streamId: Query<String>,
    //                        #[oai(name = "seek", validator(maximum(value = "86400"), minimum(value = "1")))] seek: Query<u32>) -> Json<ResultMessageData<Option<ResMsg>>> {
    //     match handler::seek(&streamId.0, seek.0).await {
    //         Err(err) => {
    //             error!("拖动失败；{}",err);
    //             Json(ResultMessageData::build_failure())
    //         }
    //         Ok(msg) => { Json(ResultMessageData::build_success(Some(msg))) }
    //     }
    // }
    // #[allow(non_snake_case)]
    // #[oai(path = "/play/back/speed", method = "get")]
    // /// 倍速播放历史视频 speed [1,2,4]
    // async fn playback_speed(&self,
    //                         #[oai(name = "streamId", validator(min_length = "32", max_length = "32"))] streamId: Query<String>,
    //                         #[oai(name = "speed", validator(maximum(value = "4"), minimum(value = "1")))] speed: Query<u8>) -> Json<ResultMessageData<Option<ResMsg>>> {
    //     match handler::speed(&streamId.0, speed.0).await {
    //         Err(err) => {
    //             error!("倍速播放失败；{}",err);
    //             Json(ResultMessageData::build_failure())
    //         }
    //         Ok(msg) => { Json(ResultMessageData::build_success(Some(msg))) }
    //     }
    // }
    // #[allow(non_snake_case)]
    // #[oai(path = "/play/back/pause", method = "get")]
    // /// 暂停播放历史视频
    // async fn playback_pause(&self,
    //                         #[oai(name = "streamId", validator(min_length = "32", max_length = "32"))] streamId: Query<String>) -> Json<ResultMessageData<Option<ResMsg>>> {
    //     match handler::pause(&streamId.0).await {
    //         Err(err) => {
    //             error!("暂停播放失败；{}",err);
    //             Json(ResultMessageData::build_failure())
    //         }
    //         Ok(msg) => { Json(ResultMessageData::build_success(Some(msg))) }
    //     }
    // }
    // #[allow(non_snake_case)]
    // #[oai(path = "/play/back/replay", method = "get")]
    // /// 恢复播放历史视频
    // async fn playback_replay(&self,
    //                          #[oai(name = "streamId", validator(min_length = "32", max_length = "32"))] streamId: Query<String>) -> Json<ResultMessageData<Option<ResMsg>>> {
    //     match handler::replay(&streamId.0).await {
    //         Err(err) => {
    //             error!("恢复播放失败；{}",err);
    //             Json(ResultMessageData::build_failure())
    //         }
    //         Ok(msg) => { Json(ResultMessageData::build_success(Some(msg))) }
    //     }
    // }
}