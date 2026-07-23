//! Per-channel bridge between a server-opened X11 channel and the local
//! X server.

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::spoof::{Rewrite, SetupRewriter};
use super::{X11Forwarding, X11Target};

/// Bridge one inbound X11 channel. Dials the local display, performs the
/// cookie swap on the connection-setup request, then pumps bytes until
/// either side closes or the session is cancelled.
pub(crate) async fn bridge_x11_channel(
    channel: russh::Channel<russh::client::Msg>,
    cfg: Arc<X11Forwarding>,
    cancel: tokio::sync::watch::Receiver<bool>,
) {
    match &cfg.target {
        X11Target::Tcp(host, port) => {
            match tokio::net::TcpStream::connect((host.as_str(), *port)).await {
                Ok(stream) => {
                    // X11 is request/response chatty (a Swing repaint is
                    // thousands of small round trips); Nagle would batch
                    // them into visible lag.
                    let _ = stream.set_nodelay(true);
                    pump(channel, stream, cfg, cancel).await;
                }
                Err(e) => tracing::warn!(
                    "X11 forward: local display {host}:{port} unreachable: {e}"
                ),
            }
        }
        #[cfg(unix)]
        X11Target::Unix(path) => match tokio::net::UnixStream::connect(path).await {
            Ok(stream) => pump(channel, stream, cfg, cancel).await,
            Err(e) => tracing::warn!(
                "X11 forward: local display socket {} unreachable: {e}",
                path.display()
            ),
        },
        #[cfg(not(unix))]
        X11Target::Unix(path) => tracing::warn!(
            "X11 forward: unix-socket display {} is not supported on this platform",
            path.display()
        ),
    }
}

async fn pump<S>(
    channel: russh::Channel<russh::client::Msg>,
    stream: S,
    cfg: Arc<X11Forwarding>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut ch_r, mut ch_w) = tokio::io::split(channel.into_stream());
    let (mut x_r, mut x_w) = tokio::io::split(stream);

    // Verify the fake cookie and rewrite the auth before ANY byte
    // reaches the X server. Always runs: the remote was always handed a
    // fake cookie, so every channel must present it, whether the local
    // display then wants the real cookie or no auth at all.
    let mut rewriter =
        SetupRewriter::new(cfg.fake_cookie.clone(), cfg.real_cookie.clone());
    let mut buf = [0u8; 1024];
    while !rewriter.is_done() {
        let n = tokio::select! {
            _ = cancel.changed() => return,
            r = ch_r.read(&mut buf) => match r {
                Ok(0) => {
                    tracing::debug!("X11 forward: channel closed before the setup request");
                    return;
                }
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!("X11 forward: reading the setup request failed: {e}");
                    return;
                }
            },
        };
        match rewriter.push(&buf[..n]) {
            Rewrite::NeedMore => continue,
            Rewrite::Reject(why) => {
                // Loud by design: a mismatch means something other than
                // the app we authorized opened this channel.
                tracing::warn!("X11 forward: rejecting channel, {why}");
                return;
            }
            Rewrite::Done(out) => {
                if let Err(e) = x_w.write_all(&out).await {
                    tracing::warn!("X11 forward: writing the setup request failed: {e}");
                    return;
                }
            }
        }
    }

    let c2x = tokio::io::copy(&mut ch_r, &mut x_w);
    let x2c = tokio::io::copy(&mut x_r, &mut ch_w);

    tokio::select! {
        _ = cancel.changed() => {}
        r = c2x => { if let Err(e) = r { tracing::debug!("X11 forward channel->display: {e}"); } }
        r = x2c => { if let Err(e) = r { tracing::debug!("X11 forward display->channel: {e}"); } }
    }
}
