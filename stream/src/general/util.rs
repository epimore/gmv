use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::thread::sleep;
use std::time::Duration;
use base::bytes::Bytes;
use futures_core::Stream;

static INDEX: AtomicUsize = AtomicUsize::new(0);

pub fn dump(file_name: &str, data: &[u8], seq: bool) -> GlobalResult<()> {
    let path = "./dump";
    std::fs::create_dir_all(path).hand_log(|msg| error!("{msg}"))?;
    if seq {
        let i = INDEX.fetch_add(1, Ordering::SeqCst);
        let mut f = File::create(format!("{path}/{file_name}-{i}.bin")).hand_log(|msg| error!("{msg}"))?;
        f.write_all(data).hand_log(|msg| error!("{msg}"))?;
    } else {
        let mut f = OpenOptions::new().create(true).write(true).append(true).open(format!("{path}/{file_name}.bin")).hand_log(|msg| error!("{msg}"))?;
        f.write_all(data).hand_log(|msg| error!("{msg}"))?;
    }
    Ok(())
}

pub struct DumpStream<S> {
    pub inner: S,
    pub name: &'static str,
}

impl<S> Stream for DumpStream<S>
where
    S: Stream<Item = Result<Bytes, std::convert::Infallible>> + Unpin,
{
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                // dump(self.name, &bytes, false).ok();
                // dump("live", &bytes, true).ok();
                Poll::Ready(Some(Ok(bytes)))
            }
            other => other,
        }
    }
}
