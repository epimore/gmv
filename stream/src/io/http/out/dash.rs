use crate::general::mp::MediaParam;
use crate::io::event_handler::{Event, EventRes, OutEvent, OutEventRes};
use crate::io::http::out::DisconnectAwareStream;
use crate::io::http::{res_401, res_404};
use crate::media::context::event::ContextEvent;
use crate::media::context::event::inner::InnerEvent;
use crate::media::context::format::MuxPacket;
use crate::media::context::format::muxer::MuxerEnum;
use crate::state::{TIME_OUT, cache};
use axum::body::Body;
use axum::response::Response;
use base::bytes::Bytes;
use base::chrono;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::tokio::sync::{broadcast, oneshot};
use base::tokio::time::timeout;
use futures_core::Stream;
use futures_util::{StreamExt, stream};
use shared::info::obj::{BaseStreamInfo, StreamPlayInfo};
use shared::info::output::OutputEnum;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

async fn handler_fmp4(stream_id: String, token: &String, addr: SocketAddr) -> Response<Body> {
    match cache::get_base_stream_info_by_stream_id(&stream_id) {
        None => res_404(),
        Some((bsi, user_count)) => {
            let ssrc = bsi.rtp_info.ssrc;
            let token = token.to_string();
            let remote_addr = addr.to_string();

            let info = StreamPlayInfo::new(
                bsi,
                remote_addr.clone(),
                token.clone(),
                OutputEnum::DashFmp4,
                user_count,
            );

            let (tx, rx) = oneshot::channel();
            let event_tx = cache::get_event_tx();

            let _ = event_tx
                .send((Event::Out(OutEvent::OnPlay(info)), Some(tx)))
                .await;

            match rx.await {
                Ok(EventRes::Out(OutEventRes::OnPlay(Some(true)))) => {
                    match cache::get_muxer_rx(&ssrc, MuxerEnum::FMp4) {
                        Ok(rx) => {
                            cache::update_token(
                                &stream_id,
                                OutputEnum::DashFmp4,
                                token.clone(),
                                true,
                                addr,
                                None,
                            );

                            let on_disconnect: Option<Box<dyn FnOnce() + Send + Sync>> =
                                Some(Box::new(move || {
                                    cache::update_token(
                                        &stream_id,
                                        OutputEnum::DashFmp4,
                                        token.clone(),
                                        false,
                                        addr,
                                        None,
                                    );
                                    if let Some((bsi, user_count)) =
                                        cache::get_base_stream_info_by_stream_id(&stream_id)
                                    {
                                        let info = StreamPlayInfo::new(
                                            bsi,
                                            remote_addr,
                                            token,
                                            OutputEnum::DashFmp4,
                                            user_count,
                                        );
                                        let _ = event_tx
                                            .try_send((Event::Out(OutEvent::OffPlay(info)), None));
                                    }
                                }));

                            send_fmp4(ssrc, rx, on_disconnect).await
                        }
                        Err(_) => res_404(),
                    }
                }
                Ok(_) => res_401(),
                Err(_) => res_404(),
            }
        }
    }
}

async fn send_fmp4(
    ssrc: u32,
    mut rx: broadcast::Receiver<Arc<MuxPacket>>,
    on_disconnect: Option<Box<dyn FnOnce() + Send + Sync>>,
) -> Response<Body> {
    // 1. 获取 CMAF init segment
    let init_segment = match get_fmp4_init(ssrc).await {
        Ok(h) => h,
        Err(_) => return res_404(),
    };

    // 2. 等待第一个关键帧 fragment（moof+mdat）
    let first_fragment = match timeout(Duration::from_millis(TIME_OUT), async {
        loop {
            match rx.recv().await {
                Ok(pkt) if pkt.is_key => break Some(pkt.data.clone()),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    })
    .await
    {
        Ok(Some(data)) => data,
        _ => return res_404(),
    };

    // 3. 组合 stream
    let init_stream = stream::once(Box::pin(async move {
        Ok::<_, std::convert::Infallible>(init_segment)
    }));

    let first_frag_stream = stream::once(Box::pin(async move {
        Ok::<_, std::convert::Infallible>(first_fragment)
    }));

    let live_stream = Fmp4Stream {
        inner: BroadcastStream::new(rx),
    };

    let full_stream = init_stream.chain(first_frag_stream).chain(live_stream);

    let wrapped = DisconnectAwareStream {
        inner: full_stream,
        on_drop: on_disconnect,
    };

    Response::builder()
        .header("Content-Type", "video/mp4")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(Body::from_stream(wrapped))
        .unwrap()
}
async fn get_fmp4_init(ssrc: u32) -> GlobalResult<Bytes> {
    let (tx, rx) = oneshot::channel();
    cache::try_publish_mpsc(&ssrc, ContextEvent::Inner(InnerEvent::CmafHeader(tx)))?;
    Ok(rx.await.hand_log(|msg| error!("{msg}"))?)
}
struct Fmp4Stream {
    inner: BroadcastStream<Arc<MuxPacket>>,
}

impl Stream for Fmp4Stream {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(pkt))) => Poll::Ready(Some(Ok(pkt.data.clone()))),
            Poll::Ready(Some(Err(_))) => Poll::Pending, // lagged
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub async fn init_segment(stream_id: String) -> Response<Body> {
    match cache::get_base_stream_info_by_stream_id(&stream_id) {
        None => res_404(),
        Some((bsi, _)) => {
            let ssrc = bsi.rtp_info.ssrc;
            match get_fmp4_init(ssrc).await {
                Ok(init) => Response::builder()
                    .header("Content-Type", "video/mp4")
                    .header("Cache-Control", "max-age=3600")
                    .body(Body::from(init))
                    .unwrap(),
                Err(_) => res_404(),
            }
        }
    }
}

pub async fn mpd_handler(stream_id: String) -> Response<Body> {
    match cache::get_base_stream_info_by_stream_id(&stream_id) {
        None => res_404(),
        Some((bsi, _)) => match get_video_param(bsi.rtp_info.ssrc).await {
            Ok(mp) => {
                let mpd = generate_mpd(&stream_id, mp);
                Response::builder()
                    .header("Content-Type", "application/dash+xml")
                    .header("Cache-Control", "no-cache")
                    .body(Body::from(mpd))
                    .unwrap()
            }
            Err(_) => res_404(),
        },
    }
}

async fn get_video_param(ssrc: u32) -> GlobalResult<MediaParam> {
    let (tx, rx) = oneshot::channel();
    cache::try_publish_mpsc(&ssrc, ContextEvent::Inner(InnerEvent::MediaParam(tx)))?;
    Ok(rx.await.hand_log(|msg| error!("{msg}"))?)
}
fn generate_mpd(stream_id: &str, mp: MediaParam) -> String {
    let mut xml = String::new();

    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push_str(&format!(
        r#"
<MPD
  xmlns="urn:mpeg:dash:schema:mpd:2011"
  profiles="urn:mpeg:dash:profile:isoff-live:2011"
  type="dynamic"
  minimumUpdatePeriod="PT0.2S"
  timeShiftBufferDepth="PT30S"
  suggestedPresentationDelay="PT3S"
  availabilityStartTime="{}"
  minBufferTime="PT0.2S">
  <ServiceDescription>
  <Latency target="400" max="800"/>
  </ServiceDescription>
"#,
        mp.availability_start_time
    ));

    xml.push_str("<Period id=\"0\" start=\"PT0S\">");
    let server_conf = cache::get_server_conf();
    let server_name = server_conf.get_name();
    // ===== Video =====
    if let Some(v) = &mp.video {
        xml.push_str(&format!(
            r#"
<AdaptationSet
  mimeType="video/mp4"
  segmentAlignment="true"
  startWithSAP="1">
  <Representation
    id="v1"
    bandwidth="{}"
    codecs="{}"
    width="{}"
    height="{}"
    frameRate="{}">
    <SegmentTemplate
      timescale="{}"
      availabilityTimeOffset="0.1"
      availabilityTimeComplete="false"
      initialization="/{}/play/{}.m4is"
      media="/{}/play/{}.m4s?seg=$Number$"/>
  </Representation>
</AdaptationSet>
"#,
            v.bandwidth,
            v.codec,
            v.width,
            v.height,
            v.frame_rate,
            v.timescale,
            server_name,
            stream_id,
            server_name,
            stream_id,
        ));
    }

    // ===== Audio =====
    if let Some(a) = &mp.audio {
        xml.push_str(&format!(
            r#"
<AdaptationSet
  mimeType="audio/mp4"
  segmentAlignment="true"
  startWithSAP="1">
  <Representation
    id="a1"
    bandwidth="{}"
    codecs="{}"
    audioSamplingRate="{}">
    <AudioChannelConfiguration
      schemeIdUri="urn:mpeg:dash:23003:3:audio_channel_configuration:2011"
      value="{}"/>
    <SegmentTemplate
      timescale="{}"
      availabilityTimeOffset="0.1"
      availabilityTimeComplete="false"
      initialization="/{}/play/{}.m4is"
      media="/{}/play/{}.m4s?seg=$Number$"/>
  </Representation>
</AdaptationSet>
"#,
            a.bandwidth,
            a.codec,
            a.sample_rate,
            a.channels,
            a.timescale,
            server_name,
            stream_id,
            server_name,
            stream_id,
        ));
    }

    xml.push_str("</Period></MPD>");
    xml
}

pub async fn segment_handler(
    stream_id: String,
    token: &String,
    addr: SocketAddr,
) -> Response<Body> {
    handler_fmp4(stream_id, token, addr).await
}
