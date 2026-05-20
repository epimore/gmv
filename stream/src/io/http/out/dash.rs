use crate::general::mp::MediaParam;
use crate::general::util::dump;
use crate::io::http::out::{DisconnectAwareStream, OutPlayKind, stream_user_token_check};
use crate::io::http::{res_401, res_404};
use crate::media::context::event::ContextEvent;
use crate::media::context::event::inner::InnerEvent;
use crate::media::context::format::MuxPacket;
use crate::media::context::format::muxer::MuxerEnum;
use crate::state::register::{DEFAULT_OFFSET_SECOND, Register};
use axum::body::Body;
use axum::response::Response;
use base::bytes::{Bytes, BytesMut};
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::tokio::sync::{broadcast, oneshot};
use futures_util::stream;
use shared::info::output::OutputEnum;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

pub async fn chunk(stream_id: Arc<str>, token: Arc<str>, addr: SocketAddr) -> Response<Body> {
    match Register::get_base_stream_info_by_stream_id(stream_id.clone()) {
        None => res_404(),
        Some(bsi) => {
            let ssrc = bsi.rtp_info.ssrc;
            match stream_user_token_check(
                OutputEnum::DashFmp4,
                bsi,
                stream_id.clone(),
                token.clone(),
                addr,
            )
            .await
            {
                OutPlayKind::Play => match Register::get_muxer_rx(&ssrc, MuxerEnum::FMp4) {
                    Ok(rx) => {
                        let on_disconnect: Option<Box<dyn FnOnce() + Send + Sync>> =
                            Some(Box::new(move || {
                                Register::listen_output_timeout(
                                    stream_id,
                                    OutputEnum::DashFmp4,
                                    token,
                                    addr,
                                    0,
                                );
                            }));
                        send_fmp4(ssrc, rx, on_disconnect).await
                    }
                    Err(_) => res_404(),
                },
                OutPlayKind::Forbid => res_401(),
                OutPlayKind::Notfound => res_404(),
            }
        }
    }
}

pub async fn mpd_handler(stream_id: Arc<str>, token: Arc<str>, addr: SocketAddr) -> Response<Body> {
    match Register::get_base_stream_info_by_stream_id(stream_id.clone()) {
        None => res_404(),
        Some(bsi) => {
            let ssrc = bsi.rtp_info.ssrc;
            match stream_user_token_check(
                OutputEnum::DashMp4,
                bsi,
                stream_id.clone(),
                token.clone(),
                addr,
            )
            .await
            {
                OutPlayKind::Play => match get_video_param(ssrc).await {
                    Ok(mp) => {
                        let mpd = generate_mpd(stream_id, mp);
                        Response::builder()
                            .header("Content-Type", "application/dash+xml")
                            .header("Cache-Control", "no-cache")
                            .body(Body::from(mpd))
                            .unwrap()
                    }
                    Err(_) => res_404(),
                },
                OutPlayKind::Forbid => res_401(),
                OutPlayKind::Notfound => res_404(),
            }
        }
    }
}

pub async fn init_segment(
    stream_id: Arc<str>,
    token: Arc<str>,
    addr: SocketAddr,
) -> Response<Body> {
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
                            0,
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
                            DEFAULT_OFFSET_SECOND,
                        );
                        match Register::get_muxer_rx(&ssrc, MuxerEnum::DashMp4) {
                            Ok(mut rx) => match rx.recv().await {
                                Ok(pkt) => Response::builder()
                                    .header("Content-Type", "video/mp4")
                                    .body(Body::from(pkt.data.clone()))
                                    .unwrap(),
                                Err(_) => res_404(),
                            },
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

    xml.push_str("</Period></MPD>");
    xml
}

async fn send_fmp4(
    ssrc: u32,
    rx: broadcast::Receiver<Arc<MuxPacket>>,
    on_disconnect: Option<Box<dyn FnOnce() + Send + Sync>>,
) -> Response<Body> {
    let wrapped = DisconnectAwareStream {
        inner: Box::pin(fmp4_stream(ssrc, rx)),
        on_drop: on_disconnect,
    };

    Response::builder()
        .header("Content-Type", "video/mp4")
        .header("Cache-Control", "no-cache")
        .body(Body::from_stream(wrapped))
        .unwrap()
}

enum Fmp4StreamState {
    Init,
    FirstKey(Bytes),
    Live,
}

struct Fmp4StreamContext {
    ssrc: u32,
    rx: broadcast::Receiver<Arc<MuxPacket>>,
    state: Fmp4StreamState,
    started: bool,
    current_epoch: Instant,
}

fn fmp4_stream(
    ssrc: u32,
    rx: broadcast::Receiver<Arc<MuxPacket>>,
) -> impl futures_core::Stream<Item = Result<Bytes, std::convert::Infallible>> {
    stream::unfold(
        Fmp4StreamContext {
            ssrc,
            rx,
            state: Fmp4StreamState::Init,
            started: false,
            current_epoch: Instant::now(),
        },
        |mut ctx| async move {
            match ctx.state {
                Fmp4StreamState::Init => {
                    let init = get_fmp4_init(ctx.ssrc).await.ok()?;
                    ctx.state = Fmp4StreamState::FirstKey(init);
                    fmp4_next_chunk(ctx).await
                }
                Fmp4StreamState::FirstKey(_) | Fmp4StreamState::Live => fmp4_next_chunk(ctx).await,
            }
        },
    )
}

async fn fmp4_next_chunk(
    mut ctx: Fmp4StreamContext,
) -> Option<(Result<Bytes, std::convert::Infallible>, Fmp4StreamContext)> {
    loop {
        let pkt = match ctx.rx.recv().await {
            Ok(pkt) => pkt,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return None,
        };

        if pkt.epoch != ctx.current_epoch {
            ctx.current_epoch = pkt.epoch;
        }

        match &ctx.state {
            Fmp4StreamState::FirstKey(init) => {
                if !pkt.is_key {
                    continue;
                }

                ctx.started = true;
                let mut out = BytesMut::new();
                out.extend_from_slice(init);
                out.extend_from_slice(&pkt.data);
                let _ = dump("fmp4", &out, false);

                ctx.state = Fmp4StreamState::Live;
                return Some((Ok(out.freeze()), ctx));
            }
            Fmp4StreamState::Live if ctx.started => {
                let _ = dump("fmp4", &pkt.data, false);
                return Some((Ok(pkt.data.clone()), ctx));
            }
            _ => continue,
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
