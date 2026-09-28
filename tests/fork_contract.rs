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
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use support::{
    cleanup_test_base, client_handshake, register_runtime_dir, register_spawned_herdr_pid,
    send_client_shell_key, unregister_spawned_herdr_pid, wait_for_socket,
};

/// Alt (crossterm `KeyModifiers::ALT.bits()`); the client-shell wire protocol
/// sends semantic keys, not raw terminal escape bytes.
const ALT_MODIFIER: u8 = 4;

fn send_alt_x(client: &mut UnixStream, pane_id: &str) -> Result<(), String> {
    send_client_shell_key(client, pane_id, 'x', ALT_MODIFIER)
}

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

fn pane_count(api_socket: &Path) -> usize {
    let response = request(
        api_socket,
        serde_json::json!({"id":"test:panes","method":"pane.list","params":{}}),
    );
    response["result"]["panes"]
        .as_array()
        .map(|panes| panes.len())
        .unwrap_or_else(|| panic!("pane.list returned no panes array: {response}"))
}

fn wait_for_pane_count(api_socket: &Path, expected: usize, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pane_count(api_socket) == expected {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
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
// FORK.md F2 - close_pane_if_idle
//
// The decision itself ("idle" means no agent entry for the focused pane in
// the client's own cached snapshot) is real key-dispatch logic that runs
// inside the `herdr` CLIENT process (`ClientShellState::close_focused_pane_if_idle`,
// `src/client/shell/input.rs`), not on the server this file otherwise drives.
// A raw socket standing in for the client (as this file does everywhere
// else) can only send already-classified wire messages - it cannot reach
// this decision, which runs *before* that classification happens. Proving
// it therefore means calling the real client entry point in-process:
// `fork_contract_close_pane_if_idle_closes_an_agent_free_pane_but_not_one_with_an_agent`
// in `src/fork_contract_tests.rs` does that, driving `ClientShellState`
// exactly the way the client binary's own input loop does
// (`handle_input_bytes`), and is the correct home for it. See that file for
// the coverage; there is no additional real-binary-driving test to add here
// for F2. The documented-fidelity-gap end-to-end test moved with the rest of
// this file's former content stays below, `#[ignore]`d, unchanged.
// ---------------------------------------------------------------------------

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
// FORK.md F1 - the jcode reporter script's JSON-RPC on the wire
// ---------------------------------------------------------------------------

/// FORK.md F1. Runs `scripts/verify_jcode_hook.py` (already committed,
/// intentionally not reimplemented here) against the real reporter asset,
/// `src/integration/assets/jcode/herdr-agent-state.sh`, over a stand-in Unix
/// socket, and asserts the actual JSON-RPC the script emits: the session
/// anchor precedes the first state report with a strictly lower seq,
/// `turn_start` reports working, `turn_end` with a non-ok status reports
/// blocked, and a session whose `parent_id` is set (a visible swarm worker)
/// reports nothing. Skips cleanly when `python3` is not on PATH rather than
/// failing, since this environment fact is outside this port's control.
#[test]
fn fork_contract_jcode_reporter_puts_correct_json_rpc_on_the_wire() {
    let python3 = which_python3();
    let Some(python3) = python3 else {
        eprintln!("skipping: python3 not found on PATH");
        return;
    };

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = manifest_dir.join("scripts/verify_jcode_hook.py");
    let hook = manifest_dir.join("src/integration/assets/jcode/herdr-agent-state.sh");
    assert!(script.exists(), "missing {script:?}");
    assert!(hook.exists(), "missing {hook:?}");

    let output = Command::new(&python3)
        .arg(&script)
        .arg(&hook)
        .output()
        .unwrap_or_else(|err| panic!("failed to run {python3:?} {script:?}: {err}"));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "verify_jcode_hook.py reported a failing check:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_for_file_contents(path: &Path, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = fs::read_to_string(path) {
            if !text.trim().is_empty() {
                return Some(text);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// FORK.md F2, documented gap ("Fidelity gap, deliberate" in that section):
/// before v0.9.0 the idle check
/// asked the server whether the pane's foreground job was the pane's own
/// shell, a real process-tree check. v0.9.0 moved key dispatch client-side,
/// where there is no process-tree visibility, only the cached snapshot's
/// agent list (see `fork_contract_close_pane_if_idle_closes_an_agent_free_pane_but_not_one_with_an_agent`
/// above for the check that replaced it). That correctly protects a
/// *recognized* agent, but this test's stand-in is a bare, unrecognized
/// `python3` script standing in for *any* foreground program, which the
/// client cannot distinguish from an idle shell and so incorrectly closes.
/// Deliberately kept `#[ignore]`d: fixing this needs a new advertised
/// endpoint method (e.g. `pane.close_if_idle`) run server-side, which is a
/// wire-protocol-contract change out of this port's scope; this test is the
/// gap's regression-in-waiting, not something to make pass here.
#[ignore = "known gap: client-side close_pane_if_idle only recognizes herdr-detected agents as busy, not an arbitrary foreground program; see doc comment above and FORK.md F2"]
#[test]
fn fork_contract_close_pane_if_idle_reaches_a_busy_pane_then_closes_it_once_idle() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");
    let script = base.join("read-alt-x.py");
    let ready_marker = base.join("reader-ready");
    let received_marker = base.join("reader-received");

    fs::create_dir_all(&base).unwrap();
    // Stands in for the agent running in the pane: it reports the raw bytes it
    // received, then exits, leaving the pane at a bare shell prompt.
    fs::write(
        &script,
        format!(
            r#"import os
import pathlib
import select
import sys
import tty

pathlib.Path({ready:?}).write_text("ready")
tty.setraw(sys.stdin.fileno())
ready_fds, _, _ = select.select([sys.stdin.fileno()], [], [], 10)
data = os.read(sys.stdin.fileno(), 32) if ready_fds else b""
pathlib.Path({received:?}).write_text(data.hex())
"#,
            ready = ready_marker.display().to_string(),
            received = received_marker.display().to_string()
        ),
    )
    .unwrap();

    let spawned = spawn_server(
        &config_home,
        &runtime_dir,
        &api_socket,
        "onboarding = false\nconfirm_close = false\n\n[keys]\nclose_pane_if_idle = \"alt+x\"\n",
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    register_runtime_dir(&runtime_dir);

    let pane_id = create_workspace_and_get_pane(&api_socket);

    // Two panes, so closing one is observable without tearing down the
    // workspace (a last-pane close takes the workspace with it).
    assert_ok(request(
        &api_socket,
        serde_json::json!({
            "id": "test:pane:split",
            "method": "pane.split",
            "params": {"target_pane_id": pane_id, "direction": "right", "focus": false}
        }),
    ));
    assert_eq!(pane_count(&api_socket), 2, "split should produce two panes");

    // Run the stand-in agent in the focused pane.
    assert_ok(request(
        &api_socket,
        serde_json::json!({
            "id": "test:pane:run",
            "method": "pane.send_input",
            "params": {
                "pane_id": pane_id,
                "text": format!("python3 {}", script.display()),
                "keys": ["Enter"]
            }
        }),
    ));
    assert!(
        wait_for_file_contents(&ready_marker, Duration::from_secs(10)).is_some(),
        "the stand-in agent never started"
    );

    let protocol = request(
        &api_socket,
        serde_json::json!({"id":"test:protocol","method":"ping","params":{}}),
    )["result"]["protocol"]
        .as_u64()
        .expect("protocol") as u32;

    wait_for_socket(&client_socket, Duration::from_secs(10));
    let mut client = UnixStream::connect(&client_socket).expect("connect client socket");
    let (server_protocol, error) = client_handshake(&mut client, protocol, 80, 24).unwrap();
    assert_eq!(server_protocol, protocol);
    assert!(error.is_none(), "client handshake failed: {error:?}");

    // --- Step 1: busy pane. The chord must reach the program. ---
    send_alt_x(&mut client, &pane_id).expect("send alt+x to busy pane");

    let received = wait_for_file_contents(&received_marker, Duration::from_secs(10))
        .expect("the program in the pane never received any key");
    assert!(
        received.trim().contains("1b78"),
        "Alt+X must reach the program running in the pane, got bytes: {received}"
    );
    assert_eq!(
        pane_count(&api_socket),
        2,
        "the pane must NOT close while a program is running in it"
    );

    // --- Step 2: the program has exited, so the pane is idle. ---
    std::thread::sleep(Duration::from_millis(1500));

    let process_info = request(
        &api_socket,
        serde_json::json!({
            "id":"test:pane:process_info",
            "method":"pane.process_info",
            "params":{"pane_id": pane_id}
        }),
    );

    send_alt_x(&mut client, &pane_id).expect("send alt+x to idle pane");

    assert!(
        wait_for_pane_count(&api_socket, 1, Duration::from_secs(10)),
        "Alt+X should close the pane once it is idle, panes still: {}, foreground job at press time: {process_info}",
        pane_count(&api_socket)
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

fn which_python3() -> Option<PathBuf> {
    for candidate in [
        "python3",
        "/opt/homebrew/bin/python3.12",
        "/usr/bin/python3",
    ] {
        if let Ok(output) = Command::new(candidate).arg("--version").output() {
            if output.status.success() {
                return Some(PathBuf::from(candidate));
            }
        }
    }
    None
}
