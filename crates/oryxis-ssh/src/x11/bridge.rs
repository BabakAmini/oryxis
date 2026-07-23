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
    // Keep the streams whole (not split): the steady-state pump below
    // needs full-duplex handles so `copy_bidirectional` can half-close one
    // direction and drain the other. Both `ChannelStream` and the local
    // TCP/unix stream are `Unpin`, so reading/writing through `&mut` here
    // and handing the same `&mut` to `copy_bidirectional` is fine.
    let mut ch = channel.into_stream();
    let mut x = stream;

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
            r = ch.read(&mut buf) => match r {
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
                // `out` carries the rewritten setup request AND any traffic
                // that arrived in the same read, so nothing is stranded in
                // the rewriter when the byte pump takes over below.
                if let Err(e) = x.write_all(&out).await {
                    tracing::warn!("X11 forward: writing the setup request failed: {e}");
                    return;
                }
            }
        }
    }

    // Steady state: pump both directions until BOTH reach EOF.
    // `copy_bidirectional` half-closes the peer's write side on each EOF
    // (a `CHANNEL_EOF` on the SSH channel, a TCP/unix FIN on the display)
    // and keeps draining the surviving direction, so a reply still in
    // flight when one side hangs up is delivered rather than dropped. A
    // plain `select!` over two one-shot copies would instead cancel the
    // survivor the moment the first direction returned, discarding both
    // its in-flight bytes and whatever it had already buffered.
    tokio::select! {
        _ = cancel.changed() => {}
        r = tokio::io::copy_bidirectional(&mut ch, &mut x) => {
            if let Err(e) = r {
                tracing::debug!("X11 forward pump: {e}");
            }
        }
    }
}
