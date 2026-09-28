use std::io;
#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::sync::{atomic::AtomicBool, atomic::Ordering, Arc};

use interprocess::local_socket::traits::{Listener as _, Stream as _};
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::ipc::LocalListener;
use crate::server::client_transport::{self, ServerEvent};

/// Accepts pending thin-client connections and starts their handshake readers.
pub(crate) fn accept_pending_client_connections(
    listener: &LocalListener,
    next_client_id: &mut u64,
    should_quit: &Arc<AtomicBool>,
    server_event_tx: &mpsc::Sender<ServerEvent>,
) -> io::Result<()> {
    // JSB-17: counts every call, regardless of caller, so a profiler window
    // reveals whether accept is still gated on main-loop passes (tracks
    // `loop.tick` almost 1:1) or on listener readiness (near zero when idle).
    crate::render_prof::event("accept.attempt");
    loop {
        if should_quit.load(Ordering::Acquire) {
            break;
        }
        match listener.accept() {
            Ok(stream) => {
                let client_id = *next_client_id;
                *next_client_id = next_client_id.saturating_add(1);

                if let Err(err) = stream.set_nonblocking(true) {
                    warn!(err = %err, "failed to set client stream nonblocking");
                    continue;
                }

                let should_quit = should_quit.clone();
                let server_event_tx = server_event_tx.clone();
                std::thread::spawn(move || {
                    if let Err(err) = client_transport::handle_client_handshake(
                        stream,
                        client_id,
                        &server_event_tx,
                        &should_quit,
                    ) {
                        debug!(client_id, err = %err, "client handshake failed");
                    }
                });
            }
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(err) => {
                error!(err = %err, "client listener accept failed");
                break;
            }
        }
    }

    Ok(())
}

/// Drains pending thin-client connections without starting handshakes.
///
/// During live handoff the old server must not let clients sit in the Unix
/// listener backlog waiting for a welcome frame that will never be sent.
pub(crate) fn reject_pending_client_connections(listener: &LocalListener) -> io::Result<()> {
    loop {
        match listener.accept() {
            Ok(_stream) => {}
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(err) => {
                error!(err = %err, "client listener reject failed");
                break;
            }
        }
    }

    Ok(())
}

/// Blocks until the listener has a pending connection or `timeout_ms` elapses.
///
/// Returns `Ok(true)` when the listener is readable, `Ok(false)` on a timeout
/// (used only to recheck `should_quit` on a bounded cadence), and `Err` on a
/// real `poll(2)` failure.
#[cfg(unix)]
fn wait_for_listener_readable(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    let mut fds = [libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }];
    loop {
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout_ms) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        return Ok(ready > 0 && fds[0].revents & libc::POLLIN != 0);
    }
}

/// Dedicated accept thread for Unix, mirroring the Windows accept thread
/// (`spawn_windows_client_accept_thread` in `server/headless.rs`): the main
/// event loop never calls `accept()` on a guess, only this thread does, and
/// only when the listener is actually readable.
///
/// The listener stays non-blocking so `accept_pending_client_connections` and
/// `reject_pending_client_connections` keep their exact WouldBlock-terminated
/// batch-drain behavior; this thread only decides *when* to call them, by
/// blocking in `poll(2)` on the listener fd instead of the main loop calling
/// them unconditionally on every pass. See JSB-17 / `FORK.md` section F7.
#[cfg(unix)]
pub(crate) fn spawn_unix_client_accept_thread(
    listener: LocalListener,
    should_quit: Arc<AtomicBool>,
    server_event_tx: mpsc::Sender<ServerEvent>,
    reject_new_clients: Arc<AtomicBool>,
) {
    // Recheck `should_quit` on this cadence even with no connection activity,
    // matching the responsiveness of the previous main-loop poll interval.
    const POLL_TIMEOUT_MS: i32 = 250;

    std::thread::spawn(move || {
        // The only variant compiled on unix; matches instead of asserting so
        // an interprocess upgrade adding another unix backend fails loudly.
        let LocalListener::UdSocket(inner) = &listener;
        let raw_fd = inner.as_fd().as_raw_fd();
        let mut next_client_id = 1_u64;
        loop {
            if should_quit.load(Ordering::Acquire) {
                break;
            }
            match wait_for_listener_readable(raw_fd, POLL_TIMEOUT_MS) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(err) => {
                    error!(err = %err, "client listener poll failed");
                    break;
                }
            }
            if should_quit.load(Ordering::Acquire) {
                break;
            }
            let result = if reject_new_clients.load(Ordering::Acquire) {
                reject_pending_client_connections(&listener)
            } else {
                accept_pending_client_connections(
                    &listener,
                    &mut next_client_id,
                    &should_quit,
                    &server_event_tx,
                )
            };
            if let Err(err) = result {
                error!(err = %err, "client listener accept thread failed");
                break;
            }
        }
    });
}
