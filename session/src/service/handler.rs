use crate::general;
use crate::general::model::PlayLiveModel;

pub async fn play_live(play_live_model: PlayLiveModel,token:String){
    let device_id = play_live_model.get_deviceId();
    let channel_id = play_live_model.get_channelId();
    let live_info = general::cache::Cache::device_map_get_live_info(device_id, channel_id);
    unimplemented!()
}