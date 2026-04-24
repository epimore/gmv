use crate::general::mp::MediaParam;
use crate::general::util::{DumpStream, dump};
use crate::io::http::out::{stream_user_token_check, DisconnectAwareStream, OutPlayKind};
use crate::io::http::{res_401, res_404};
use crate::media::context::event::ContextEvent;
use crate::media::context::event::inner::InnerEvent;
use crate::media::context::format::MuxPacket;
use crate::media::context::format::muxer::MuxerEnum;
use crate::state::event::{Event, EventRes, OutEvent, OutEventRes};
use crate::state::register::{Register, DEFAULT_OFFSET_SECOND};
use axum::body::Body;
use axum::response::Response;
use base::bytes::{Bytes, BytesMut};
use base::cache::{CachedValue, CommonCache};
use base::chrono::{Local, SecondsFormat};
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::tokio::sync::oneshot::error::RecvError;
use base::tokio::sync::{broadcast, oneshot};
use base::tokio::time::timeout;
use base::{chrono, tokio};
use futures_core::Stream;
use futures_util::{StreamExt, future, stream};
use shared::info::obj::{BaseStreamInfo, StreamPlayInfo};
use shared::info::output::OutputEnum;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio_stream::wrappers::BroadcastStream;

pub async fn chunk(stream_id: Arc<str>, token: Arc<str>, addr: SocketAddr) -> Response<Body> {
    match Register::get_base_stream_info_by_stream_id(stream_id.clone()) {
        None => res_404(),
        Some(bsi) => {
            let ssrc = bsi.rtp_info.ssrc;
            match stream_user_token_check(OutputEnum::DashFmp4,bsi,stream_id.clone(),token.clone(),addr).await {
                OutPlayKind::Play => {
                    match Register::get_muxer_rx(&ssrc, MuxerEnum::Flv) {
                        Ok(rx) => {
                            let on_disconnect: Option<Box<dyn FnOnce() + Send + Sync>> =
                                Some(Box::new(move || {
                                    Register::listen_output_timeout(
                                        stream_id,
                                        OutputEnum::DashFmp4,
                                        token,
                                        addr,
                                        0
                                    );
                                }));
                            send_fmp4(ssrc, rx, on_disconnect).await
                        }
                        Err(_) => res_404(),
                    }
                }
                OutPlayKind::Forbid => {res_401()}
                OutPlayKind::Notfound => {res_404()}
            }
        }
    }
}

pub async fn mpd_handler(stream_id: Arc<str>, token: Arc<str>, addr: SocketAddr) -> Response<Body> {
    match Register::get_base_stream_info_by_stream_id(stream_id.clone()) {
        None => res_404(),
        Some(bsi) => {
            let ssrc = bsi.rtp_info.ssrc;
            match stream_user_token_check(OutputEnum::DashMp4,bsi,stream_id.clone(),token.clone(),addr).await {
                OutPlayKind::Play => {
                    match get_video_param(ssrc).await {
                        Ok(mp) => {
                            let mpd = generate_mpd(stream_id, mp);
                            Response::builder()
                                .header("Content-Type", "application/dash+xml")
                                .header("Cache-Control", "no-cache")
                                .body(Body::from(mpd))
                                .unwrap()
                        }
                        Err(_) => res_404(),
                    }
                }
                OutPlayKind::Forbid => {res_401()}
                OutPlayKind::Notfound => {res_404()}
            }
        }
    }
}

pub async fn init_segment(stream_id: Arc<str>, token: Arc<str>, addr: SocketAddr) -> Response<Body> {
    match Register::get_base_stream_info_by_stream_id(stream_id.clone()) {
        None => res_404(),
        Some(bsi) => {
            let ssrc = bsi.rtp_info.ssrc;
            if Register::check_token(&(token.clone(), stream_id.clone())) {
                match Register::insert_out_token(
                    stream_id.clone(),
                    OutputEnum::DashMp4,
                    token.clone(),
                ) {
                    Ok(_) => {
                        Register::listen_output_timeout(
                            stream_id,
                            OutputEnum::DashMp4,
                            token,
                            addr,
                            0
                        );
                        match get_dash_mp4_init(ssrc).await {
                            Ok(init) => Response::builder()
                                .header("Content-Type", "video/mp4")
                                .header("Cache-Control", "max-age=3600")
                                .body(Body::from(init))
                                .unwrap(),
                            Err(_) => res_404(),
                        }
                    }
                    Err(_) => res_404(),
                }
            } else {
                res_401()
            }
        }
    }
}

pub async fn segment(stream_id: Arc<str>, token: Arc<str>, addr: SocketAddr) -> Response<Body> {
    match Register::get_base_stream_info_by_stream_id(stream_id.clone()) {
        None => res_404(),
        Some(bsi) => {
            let ssrc = bsi.rtp_info.ssrc;
            if Register::check_token(&(token.clone(), stream_id.clone())) {
                match Register::insert_out_token(
                    stream_id.clone(),
                    OutputEnum::DashMp4,
                    token.clone(),
                ) {
                    Ok(_) => {
                        Register::listen_output_timeout(
                            stream_id,
                            OutputEnum::DashMp4,
                            token,
                            addr,
                            DEFAULT_OFFSET_SECOND
                        );
                        match Register::get_muxer_rx(&ssrc, MuxerEnum::DashMp4) {
                            Ok(mut rx) => {
                                match rx.recv().await {
                                    Ok(pkt) => {
                                        // dump("dash_seg",&pkt.data,true).unwrap();
                                        Response::builder()
                                            .header("Content-Type", "video/mp4")
                                            .body(Body::from(pkt.data.clone()))
                                            .unwrap()
                                    }
                                    Err(_) => res_404(),
                                }
                            }
                            _ => res_404(),
                        }
                    }
                    Err(_) => res_404(),
                }
            } else {
                res_401()
            }
        }
    }
}

async fn get_video_param(ssrc: u32) -> GlobalResult<MediaParam> {
    let (tx, rx) = oneshot::channel();
    Register::try_publish_mpsc(ssrc, ContextEvent::Inner(InnerEvent::MediaParam(tx)))?;
    Ok(rx.await.hand_log(|msg| error!("{msg}"))?)
}

fn generate_mpd(stream_id: Arc<str>, mp: MediaParam) -> String {
    let mut xml = String::new();

    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push_str(
        r#"
<MPD
  xmlns="urn:mpeg:dash:schema:mpd:2011"
  profiles="urn:mpeg:dash:profile:isoff-live:2011"
  type="static"
  mediaPresentationDuration="PT1H"
  minBufferTime="PT1S">
"#,
    );

    xml.push_str("<Period id=\"0\" start=\"PT0S\">");
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
      duration="{}"
      startNumber="1"
      presentationTimeOffset="0"
      initialization="{}.m4it"
      media="{}.m4s?seg=seg-$Number$">
</SegmentTemplate>
  </Representation>
</AdaptationSet>
"#,
            v.bandwidth,
            v.codec,
            v.width,
            v.height,
            v.frame_rate,
            v.timescale,
            v.timescale * 2,
            stream_id,
            stream_id,
        ));
    }

    // ===== Audio =====
    /*if let Some(a) = &mp.audio {
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
    }*/

    xml.push_str("</Period></MPD>");
    xml
}

async fn send_fmp4(
    ssrc: u32,
    rx: broadcast::Receiver<Arc<MuxPacket>>,
    on_disconnect: Option<Box<dyn FnOnce() + Send + Sync>>,
) -> Response<Body> {
    match get_fmp4_init(ssrc).await {
        Ok(init_segment) => {
            let stream = Fmp4Stream::new(rx, init_segment);
            let wrapped = DisconnectAwareStream {
                inner: stream,
                on_drop: on_disconnect,
            };
            Response::builder()
                .header("Content-Type", "video/mp4")
                .header("Cache-Control", "no-cache")
                .body(Body::from_stream(wrapped))
                .unwrap()
        },
        Err(_) => res_404(),
    }
}

struct Fmp4Stream {
    inner: BroadcastStream<Arc<MuxPacket>>,
    init: Bytes,
    started: bool,
    current_epoch: Instant,
}

impl Fmp4Stream {
    pub fn new(rx: broadcast::Receiver<Arc<MuxPacket>>, init: Bytes) -> Self {
        Self {
            inner: BroadcastStream::new(rx),
            init,
            started: false,
            current_epoch: Instant::now(),
        }
    }
}

impl Stream for Fmp4Stream {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(pkt))) => {
                    //当dts回退重新构建muxer时，两种方式。
                    if pkt.epoch != self.current_epoch {
                        //单选1.断开连接，让客户端重新连接
                        // if self.started {
                        //     return Poll::Ready(None);
                        // }
                        //1. end
                        self.current_epoch = pkt.epoch;
                        //单选2. 重发init片段
                        // self.started = false;
                        //2. end
                    }

                    if !self.started {
                        if !pkt.is_key {
                            continue;
                        }

                        self.started = true;

                        let mut out = BytesMut::new();
                        out.extend_from_slice(&self.init);
                        out.extend_from_slice(&pkt.data);
                        let _ = dump("fmp4", &out, false);
                        return Poll::Ready(Some(Ok(out.freeze())));
                    }
                    let _ = dump("fmp4", &pkt.data, false);
                    return Poll::Ready(Some(Ok(pkt.data.clone())));
                }
                Poll::Ready(Some(Err(_))) => {
                    continue;
                }
                Poll::Ready(None) => {
                    return Poll::Ready(None);
                } // 发送者关闭
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
async fn get_fmp4_init(ssrc: u32) -> GlobalResult<Bytes> {
    let (tx, rx) = oneshot::channel();
    Register::try_publish_mpsc(ssrc, ContextEvent::Inner(InnerEvent::Fmp4Header(tx)))?;
    Ok(rx.await.hand_log(|msg| error!("{msg}"))?)
}

async fn get_dash_mp4_init(ssrc: u32) -> GlobalResult<Bytes> {
    let (tx, rx) = oneshot::channel();
    Register::try_publish_mpsc(ssrc, ContextEvent::Inner(InnerEvent::DashMp4Header(tx)))?;
    Ok(rx.await.hand_log(|msg| error!("{msg}"))?)
}
