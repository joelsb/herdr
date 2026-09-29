//! Fork contract tests: process/CLI-driving proof that every feature in
//! `FORK.md` survives an upstream merge.
//!
//! Run `cargo nextest run fork_contract` to run exactly this set (plus the
//! sibling crate-internal set in `src/fork_contract_tests.rs`, which the same
//! prefix also selects). A red test here means a fork feature was lost in a
//! merge: go read the matching `FORK.md` section (`## F<n> - ...`) for what
//! the feature is, how it is supposed to work, and the replication hazard
//! that most likely caused the loss.
//!
//! Every test in this file, and in `src/fork_contract_tests.rs`, is named
//! `fork_contract_*`. Fork tests must never move back into an upstream-owned
//! test file (anything upstream also has, e.g. `src/integration/tests.rs`,
//! `src/detect/mod.rs`, `src/terminal/state.rs`, `src/client/shell/tests/*`):
//! upstream owns those files, so a merge can delete a hunk inside one and
//! silently take a fork assertion with it. `tests/fork_contract.rs` and
//! `src/fork_contract_tests.rs` do not exist upstream, so a merge can only
//! ever add to them, never delete them out from under a fork feature.

mod support;

use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use support::{
    cleanup_test_base, client_handshake, register_runtime_dir, register_spawned_herdr_pid,
    unregister_spawned_herdr_pid, wait_for_socket,
};

struct SpawnedHerdr {
    _master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl Drop for SpawnedHerdr {
    fn drop(&mut self) {
        let pid = self.child.process_id();
        let _ = self.child.kill();
        unregister_spawned_herdr_pid(pid);
    }
}

fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn unique_test_dir() -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!("/tmp/hfc-{}-{n}", std::process::id()))
}

/// Write a config where the binary under test will actually look for it.
///
/// `app_dir_name()` is `herdr-dev` in a debug build and `herdr` in a release
/// build, and tests are compiled in debug, so write both names.
fn write_config(config_home: &Path, contents: &str) {
    for dir_name in ["herdr", "herdr-dev"] {
        let dir = config_home.join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), contents).unwrap();
    }
}

fn spawn_server(
    config_home: &Path,
    runtime_dir: &Path,
    api_socket: &Path,
    config: &str,
) -> SpawnedHerdr {
    spawn_server_with_env(config_home, runtime_dir, api_socket, config, &[])
}

fn spawn_server_with_env(
    config_home: &Path,
    runtime_dir: &Path,
    api_socket: &Path,
    config: &str,
    extra_env: &[(&str, &str)],
) -> SpawnedHerdr {
    fs::create_dir_all(runtime_dir).unwrap();
    write_config(config_home, config);

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_herdr"));
    cmd.arg("server");
    cmd.env("XDG_CONFIG_HOME", config_home);
    cmd.env("XDG_RUNTIME_DIR", runtime_dir);
    cmd.env("HERDR_SOCKET_PATH", api_socket);
    cmd.env(
        "HERDR_CLIENT_SOCKET_PATH",
        runtime_dir.join("herdr-client.sock"),
    );
    cmd.env("SHELL", "/bin/sh");
    for (key, value) in extra_env {
        cmd.env(key, value);
    }

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_herdr_pid(child.process_id());
    SpawnedHerdr {
        _master: pair.master,
        child,
    }
}

fn try_request(
    socket_path: &Path,
    request: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let mut payload = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    payload.push(b'\n');
    stream.write_all(&payload).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).map_err(|e| e.to_string())?;
    let line = buf.lines().next().ok_or("empty api response")?;
    serde_json::from_str(line).map_err(|e| format!("{e}: {line}"))
}

fn request(socket_path: &Path, req: serde_json::Value) -> serde_json::Value {
    try_request(socket_path, req).unwrap_or_else(|err| panic!("{err}"))
}

fn assert_ok(response: serde_json::Value) {
    assert!(
        response.get("result").is_some(),
        "api request failed: {response}"
    );
}

fn wait_for_pane_title(api_socket: &Path, pane_id: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(response) = try_request(
            api_socket,
            serde_json::json!({"id":"test:panes","method":"pane.list","params":{}}),
        ) {
            if let Some(panes) = response["result"]["panes"].as_array() {
                if let Some(pane) = panes
                    .iter()
                    .find(|pane| pane["pane_id"].as_str() == Some(pane_id))
                {
                    if let Some(title) = pane["terminal_title"].as_str() {
                        if !title.is_empty() {
                            return Some(title.to_string());
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    None
}

fn create_workspace_and_get_pane(api_socket: &Path) -> String {
    let created = request(
        api_socket,
        serde_json::json!({
            "id": "test:workspace:create",
            "method": "workspace.create",
            "params": {"cwd": "/tmp", "focus": true}
        }),
    );
    created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("pane id")
        .to_string()
}

fn get_pane(api_socket: &Path, pane_id: &str) -> serde_json::Value {
    request(
        api_socket,
        serde_json::json!({
            "id": "test:pane:get",
            "method": "pane.get",
            "params": {"pane_id": pane_id}
        }),
    )
}

// ---------------------------------------------------------------------------
// FORK.md F3 - pane titles survive a live handoff
// ---------------------------------------------------------------------------

/// FORK.md F3. A live handoff must carry each pane's terminal title through
/// to the new server. Titles are set by an OSC sequence the *previous* server
/// already consumed, so an imported pane is never "dirty" and the normal
/// title sync skips it; before the fix, every pane came out of a handoff with
/// no title. Observed live 2026-08-27: nine agents went nameless after a
/// handoff, while their sessions stayed intact.
#[test]
fn fork_contract_live_handoff_keeps_pane_terminal_titles() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");

    fs::create_dir_all(&base).unwrap();
    let spawned = spawn_server(
        &config_home,
        &runtime_dir,
        &api_socket,
        "onboarding = false\nconfirm_close = false\n",
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    register_runtime_dir(&runtime_dir);

    let pane_id = create_workspace_and_get_pane(&api_socket);

    // Set a title the way a real agent does: an OSC 2 sequence from the
    // program inside the pane.
    assert_ok(request(
        &api_socket,
        serde_json::json!({
            "id": "test:pane:title",
            "method": "pane.send_input",
            "params": {
                "pane_id": pane_id,
                "text": "printf '\\033]2;handoff-title-probe\\007'",
                "keys": ["Enter"]
            }
        }),
    ));

    let title_before = wait_for_pane_title(&api_socket, &pane_id, Duration::from_secs(10));
    assert_eq!(
        title_before.as_deref(),
        Some("handoff-title-probe"),
        "the pane should carry its title before the handoff"
    );

    assert_ok(request(
        &api_socket,
        serde_json::json!({"id":"test:handoff","method":"server.live_handoff","params":{}}),
    ));
    std::thread::sleep(Duration::from_secs(2));
    wait_for_socket(&api_socket, Duration::from_secs(15));

    let title_after = wait_for_pane_title(&api_socket, &pane_id, Duration::from_secs(10));
    assert_eq!(
        title_after.as_deref(),
        Some("handoff-title-probe"),
        "the title must survive a live handoff; losing it blanks every agent name in the sidebar"
    );

    let _ = request(
        &api_socket,
        serde_json::json!({"id":"test:stop","method":"server.stop","params":{}}),
    );
    drop(spawned);
    cleanup_test_base(&base);
}

// ---------------------------------------------------------------------------
// FORK.md F6 - a moved pane's old public id survives a live handoff
// ---------------------------------------------------------------------------

/// FORK.md F6. `pane.move` across workspaces gives a pane a new public id
/// and records `old -> new` in `App::state.public_pane_id_aliases`, so an
/// agent still reporting to its baked-in `HERDR_PANE_ID` keeps resolving.
/// `HandoffManifest` did not carry that map, so a live handoff after a move
/// dropped it and the old id came back `pane_not_found`. Observed live in a
/// throwaway session with herdr 0.9.1: before handoff `env=w1:p2
/// resolves="pane_id":"w2:p2"`, after handoff `env=w1:p2
/// resolves="code":"pane_not_found"`.
#[test]
fn fork_contract_moved_pane_old_id_resolves_after_live_handoff() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");

    fs::create_dir_all(&base).unwrap();
    let spawned = spawn_server(
        &config_home,
        &runtime_dir,
        &api_socket,
        "onboarding = false\nconfirm_close = false\n",
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    register_runtime_dir(&runtime_dir);

    let old_pane_id = create_workspace_and_get_pane(&api_socket);
    let workspace_b_pane_id = create_workspace_and_get_pane(&api_socket);
    let workspace_b = get_pane(&api_socket, &workspace_b_pane_id)["result"]["pane"]["workspace_id"]
        .as_str()
        .expect("workspace b id")
        .to_string();

    let moved = request(
        &api_socket,
        serde_json::json!({
            "id": "test:pane:move",
            "method": "pane.move",
            "params": {
                "pane_id": old_pane_id,
                "destination": {"type": "new_tab", "workspace_id": workspace_b}
            }
        }),
    );
    assert_ok(moved.clone());
    let new_pane_id = moved["result"]["move_result"]["pane"]["pane_id"]
        .as_str()
        .expect("moved pane id")
        .to_string();
    assert_ne!(
        old_pane_id, new_pane_id,
        "a cross-workspace move must assign the pane a new public id"
    );

    let resolved_before = get_pane(&api_socket, &old_pane_id);
    assert_eq!(
        resolved_before["result"]["pane"]["pane_id"].as_str(),
        Some(new_pane_id.as_str()),
        "the old id must resolve to the pane's new id before any handoff: {resolved_before}"
    );

    assert_ok(request(
        &api_socket,
        serde_json::json!({"id":"test:handoff","method":"server.live_handoff","params":{}}),
    ));
    std::thread::sleep(Duration::from_secs(2));
    wait_for_socket(&api_socket, Duration::from_secs(15));

    let resolved_after = get_pane(&api_socket, &old_pane_id);
    assert_eq!(
        resolved_after["result"]["pane"]["pane_id"].as_str(),
        Some(new_pane_id.as_str()),
        "the old id must still resolve to the pane's new id after a live handoff: {resolved_after}"
    );

    let _ = request(
        &api_socket,
        serde_json::json!({"id":"test:stop","method":"server.stop","params":{}}),
    );
    drop(spawned);
    cleanup_test_base(&base);
}

// ---------------------------------------------------------------------------
// FORK.md F4 - idle aging: PaneInfo.state_age_seconds reported over the API
// ---------------------------------------------------------------------------

/// FORK.md F4. `PaneInfo.state_age_seconds` must actually appear on the wire
/// for a pane whose agent state was just reported, additive so an older peer
/// simply omits the field. This is the API half of idle aging: the client
/// shell's bucket/glyph selection (covered separately in
/// `src/fork_contract_tests.rs`) is worthless if the server never sends the
/// elapsed time to begin with.
#[test]
fn fork_contract_pane_info_reports_state_age_seconds_over_the_api() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");

    fs::create_dir_all(&base).unwrap();
    let spawned = spawn_server(
        &config_home,
        &runtime_dir,
        &api_socket,
        "onboarding = false\nconfirm_close = false\n",
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    register_runtime_dir(&runtime_dir);

    let pane_id = create_workspace_and_get_pane(&api_socket);

    assert_ok(request(
        &api_socket,
        serde_json::json!({
            "id": "test:pane:report_agent",
            "method": "pane.report_agent",
            "params": {
                "pane_id": pane_id,
                "source": "herdr:fork-contract-test",
                "agent": "fork-contract-agent",
                "state": "idle"
            }
        }),
    ));

    let response = get_pane(&api_socket, &pane_id);
    let age = response["result"]["pane"]["state_age_seconds"].as_u64();
    assert!(
        age.is_some(),
        "pane.get must report state_age_seconds once an agent state has been reported: {response}"
    );

    let _ = request(
        &api_socket,
        serde_json::json!({"id":"test:stop","method":"server.stop","params":{}}),
    );
    drop(spawned);
    cleanup_test_base(&base);
}

// ---------------------------------------------------------------------------
// Fix, 2026-09-28 - a silently dropped agent state report names its reason
// (docs/findings/2026-08-28-suppression-latch-drops-agent-reports.md, 0039)
// ---------------------------------------------------------------------------

/// Finding 0039, real-binary half: drives a real `herdr:pi`/`pi` report
/// through the actual socket, using a real foreground process the pane's own
/// process-tree detector identifies as `pi`
/// (`identify_agent_in_job_detects_shell_wrapped_pi` in `src/detect/mod.rs`)
/// so `agent.explain` takes the full-lifecycle-hook-authority branch exactly
/// as it would for a real `pi` session, without installing one. Reports a
/// state at seq 11, then again at seq 10 (not newer): the second report must
/// be dropped and `agent.explain --json`'s `skipped_update_reason` must name
/// why, not read `null` the way it did before this fix.
#[test]
fn fork_contract_agent_explain_names_why_a_report_was_dropped() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");

    fs::create_dir_all(&base).unwrap();
    let spawned = spawn_server(
        &config_home,
        &runtime_dir,
        &api_socket,
        "onboarding = false\nconfirm_close = false\n",
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    register_runtime_dir(&runtime_dir);

    let pane_id = create_workspace_and_get_pane(&api_socket);

    // A real foreground process named exactly `pi`: the pane's own
    // process-tree detector identifies it as agent `pi` from the script
    // path alone, the same shape `identify_agent_in_job_detects_shell_wrapped_pi`
    // covers, so `detected_agent` becomes `Some(Agent::Pi)` without installing
    // a real coding agent.
    let bin_dir = base.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let script = bin_dir.join("pi");
    fs::write(&script, "#!/bin/sh\nsleep 60\n").unwrap();
    let mut perms = fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).unwrap();

    assert_ok(request(
        &api_socket,
        serde_json::json!({
            "id": "test:pane:run",
            "method": "pane.send_input",
            "params": {
                "pane_id": pane_id,
                "text": script.display().to_string(),
                "keys": ["Enter"]
            }
        }),
    ));

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut detected = false;
    while Instant::now() < deadline {
        let pane = get_pane(&api_socket, &pane_id);
        if pane["result"]["pane"]["agent"].as_str() == Some("pi") {
            detected = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    assert!(
        detected,
        "the pane's foreground `pi` script must be detected as agent pi"
    );

    // Anchor the session the way a real full-lifecycle hook does:
    // `pane.report_agent_session` before the first state report.
    assert_ok(request(
        &api_socket,
        serde_json::json!({
            "id": "test:pane:report_agent_session",
            "method": "pane.report_agent_session",
            "params": {
                "pane_id": pane_id,
                "source": "herdr:pi",
                "agent": "pi",
                "agent_session_id": "fork-contract-session",
                "session_start_source": "new",
                "seq": 10
            }
        }),
    ));

    // The accepted report.
    assert_ok(request(
        &api_socket,
        serde_json::json!({
            "id": "test:pane:report_agent:n",
            "method": "pane.report_agent",
            "params": {
                "pane_id": pane_id,
                "source": "herdr:pi",
                "agent": "pi",
                "state": "working",
                "agent_session_id": "fork-contract-session",
                "seq": 11
            }
        }),
    ));

    // A report with a seq that is not newer than the one just accepted must
    // be dropped, and `agent.explain` must say why.
    assert_ok(request(
        &api_socket,
        serde_json::json!({
            "id": "test:pane:report_agent:n_minus_1",
            "method": "pane.report_agent",
            "params": {
                "pane_id": pane_id,
                "source": "herdr:pi",
                "agent": "pi",
                "state": "working",
                "agent_session_id": "fork-contract-session",
                "seq": 10
            }
        }),
    ));

    let explain = request(
        &api_socket,
        serde_json::json!({
            "id": "test:agent:explain",
            "method": "agent.explain",
            "params": {"target": pane_id}
        }),
    );
    assert_eq!(
        explain["result"]["explain"]["skipped_update_reason"].as_str(),
        Some("accept_hook_report_seq_rejected"),
        "a dropped report must name its reason, not disappear silently: {explain}"
    );

    let _ = request(
        &api_socket,
        serde_json::json!({"id":"test:stop","method":"server.stop","params":{}}),
    );
    drop(spawned);
    cleanup_test_base(&base);
}

// ---------------------------------------------------------------------------
// JSB-17 - accept moved off the main loop
// ---------------------------------------------------------------------------

/// Sums one `render_prof` counter (`src/render_prof.rs`) across every
/// `render.prof` window logged in `herdr-server.log`. Each window line looks
/// like `...counters=loop.tick=75,accept.attempt=3,... durations=...`; this
/// pulls `name=value` out of the `counters=` segment on every matching line.
fn sum_render_prof_counter(log: &str, name: &str) -> u64 {
    let mut total = 0u64;
    for line in log.lines() {
        let Some(counters_start) = line.find("counters=") else {
            continue;
        };
        let rest = &line[counters_start + "counters=".len()..];
        let counters = rest.split(" durations=").next().unwrap_or(rest);
        for entry in counters.split(',') {
            if let Some(value) = entry.strip_prefix(name).and_then(|s| s.strip_prefix('=')) {
                if let Ok(n) = value.trim().parse::<u64>() {
                    total += n;
                }
            }
        }
    }
    total
}

/// JSB-17. Before the fix, `accept_client_connections()` called
/// `accept_pending_client_connections()` (a non-blocking `accept()`) on
/// every single pass of the headless main loop, whether or not a connection
/// was pending. A render frame delivered to an attached client wakes the
/// loop once for the render itself and once more for
/// `ServerEvent::ClientWriterDrained` releasing writer backpressure, so a
/// busy but otherwise ordinary render loop (one real pane producing rapid
/// small output - the shape of an agent spinner, not raw byte volume) drove
/// many main-loop passes, and the accept call rode along on every one of
/// them for free. This test forces that busy render loop with one real
/// attached client and one real pane writing output every 50ms, then reads
/// the `HERDR_RENDER_PROF` counters `accept.attempt` (`client_accept.rs`,
/// fires on every call to `accept_pending_client_connections`, wherever it
/// is invoked from) and `loop.tick` (`server/headless.rs`, fires once per
/// main-loop pass) out of `herdr-server.log`. Before the fix,
/// `accept.attempt` tracks `loop.tick` almost 1:1; after the fix, accept
/// only happens on the dedicated accept thread when the listener is
/// actually readable, so `accept.attempt` stays near zero no matter how busy
/// rendering gets.
#[test]
fn fork_contract_busy_render_loop_does_not_drive_per_pass_accept_calls() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    fs::create_dir_all(&base).unwrap();
    let spawned = spawn_server_with_env(
        &config_home,
        &runtime_dir,
        &api_socket,
        "onboarding = false\nconfirm_close = false\n",
        &[("HERDR_RENDER_PROF", "1")],
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    register_runtime_dir(&runtime_dir);

    let pane_id = create_workspace_and_get_pane(&api_socket);

    let protocol = request(
        &api_socket,
        serde_json::json!({"id":"test:protocol","method":"ping","params":{}}),
    )["result"]["protocol"]
        .as_u64()
        .expect("protocol") as u32;

    // A render frame only reaches `ServerEvent::ClientWriterDrained` (and so
    // wakes the loop a second time per render) when a client is actually
    // attached and being streamed to.
    wait_for_socket(&client_socket, Duration::from_secs(10));
    let mut client = UnixStream::connect(&client_socket).expect("connect client socket");
    let (server_protocol, error) = client_handshake(&mut client, protocol, 80, 24).unwrap();
    assert_eq!(server_protocol, protocol);
    assert!(error.is_none(), "client handshake failed: {error:?}");

    assert_ok(request(
        &api_socket,
        serde_json::json!({
            "id": "test:pane:busy",
            "method": "pane.send_input",
            "params": {
                "pane_id": pane_id,
                "text": "while :; do printf 'x%s\\r' $RANDOM; sleep 0.05; done",
                "keys": ["Enter"]
            }
        }),
    ));

    // A couple of one-second profiler windows' worth of a real busy render
    // loop; long enough for the counters to be load-bearing, short enough
    // to keep the test fast.
    std::thread::sleep(Duration::from_secs(3));

    let log_path = config_home.join("herdr-dev").join("herdr-server.log");
    let log = fs::read_to_string(&log_path).unwrap_or_default();
    let loop_ticks = sum_render_prof_counter(&log, "loop.tick");
    let accept_attempts = sum_render_prof_counter(&log, "accept.attempt");

    let _ = request(
        &api_socket,
        serde_json::json!({"id":"test:stop","method":"server.stop","params":{}}),
    );
    drop(client);
    drop(spawned);
    cleanup_test_base(&base);

    assert!(
        loop_ticks > 20,
        "test setup did not generate a busy render loop (loop.tick={loop_ticks}); \
         cannot prove anything about accept without one, log follows:\n{log}"
    );
    assert!(
        accept_attempts <= 2,
        "accept() must not ride along on every busy-loop pass: loop.tick={loop_ticks} \
         accept.attempt={accept_attempts}; JSB-17 regressed if accept.attempt tracks loop.tick"
    );
}
