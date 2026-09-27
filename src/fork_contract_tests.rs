//! Fork contract tests: crate-internal proof that every feature in `FORK.md`
//! survives an upstream merge.
//!
//! Run `cargo nextest run fork_contract` to run exactly this set (plus the
//! sibling binary/CLI-driving set in `tests/fork_contract.rs`, which the same
//! prefix also selects). A red test here means a fork feature was lost in a
//! merge: go read the matching `FORK.md` section (`## F<n> - ...`) for what
//! the feature is, how it is supposed to work, and the replication hazard
//! that most likely caused the loss.
//!
//! Every test here, and in `tests/fork_contract.rs`, is named `fork_contract_*`.
//! Fork tests must never move back into an upstream-owned file (anything
//! upstream also has, e.g. `src/integration/tests.rs`, `src/detect/mod.rs`,
//! `src/terminal/state.rs`, `src/client/shell/tests/*`): upstream owns those
//! files, so a merge can delete a hunk inside one and silently take a fork
//! assertion with it. This file (wired in from `src/main.rs` with one line)
//! does not exist upstream, so a merge can only ever add to it, never delete
//! it out from under a fork feature.
//!
//! A handful of small visibility bumps (`pub(super)` -> `pub(crate)`, and a
//! few `#[cfg(test)] pub(crate) use` re-exports) were made in the product
//! modules this file reaches into, purely so these tests can see the items
//! they assert on from outside those modules. None of them change what the
//! product code does; each is commented at its site with why it exists.

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    // -----------------------------------------------------------------
    // FORK.md F1 - jcode is a registry integration target
    // -----------------------------------------------------------------

    /// The commands jcode would run for `event`, read back from the config it
    /// actually wrote. Parsed with the real TOML parser so a config that only
    /// looks right cannot pass.
    fn jcode_hook_commands(config: &str, event: &str) -> Vec<String> {
        let parsed: toml::Value = toml::from_str(config).expect("jcode config must be valid TOML");
        match parsed.get("hooks").and_then(|hooks| hooks.get(event)) {
            Some(toml::Value::String(command)) => vec![command.clone()],
            Some(toml::Value::Array(commands)) => commands
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    }

    fn unique_jcode_base() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "herdr-fork-contract-jcode-{}-{n}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn clear_jcode_env() {
        std::env::remove_var(crate::integration::JCODE_HOME_ENV_VAR);
    }

    /// FORK.md F1: jcode is a real registry integration target. `install_jcode`
    /// writes the reporter hook and registers it for every event in `[hooks]`
    /// of jcode's own `config.toml`, preserving whatever the user already had
    /// there. If this goes red, herdr stopped wiring itself into jcode at
    /// install time and every jcode pane goes back to `agent_status=unknown`.
    #[test]
    fn fork_contract_install_jcode_writes_hook_and_registers_every_event() {
        let _lock = crate::integration::integration_env_lock();
        let base = unique_jcode_base();
        let home = base.join("home");
        let jcode_dir = home.join(".jcode");
        std::fs::create_dir_all(&jcode_dir).unwrap();
        std::fs::write(
            jcode_dir.join("config.toml"),
            "model = \"claude\"\n\n[hooks]\nturn_end = \"echo keep\"\npre_tool_timeout_ms = 5000\n",
        )
        .unwrap();
        std::env::set_var("HOME", &home);
        clear_jcode_env();

        let installed = crate::integration::install_jcode().unwrap();
        let config = std::fs::read_to_string(&installed.config_path).unwrap();
        let command = crate::integration::hook_command(&installed.hook_path, None);

        assert_eq!(
            installed.hook_path,
            jcode_dir
                .join("hooks")
                .join(crate::integration::JCODE_HOOK_INSTALL_NAME)
        );
        assert_eq!(
            std::fs::read_to_string(&installed.hook_path).unwrap(),
            crate::integration::JCODE_HOOK_ASSET
        );
        for event in crate::integration::JCODE_HOOK_EVENTS {
            assert!(
                jcode_hook_commands(&config, event).contains(&command),
                "jcode config is missing the herdr command for {event}"
            );
        }
        // An unrelated command the user configured must survive and keep
        // running first, so herdr never displaces an existing dispatcher.
        assert_eq!(
            jcode_hook_commands(&config, "turn_end"),
            vec!["echo keep".to_string(), command.clone()]
        );

        std::env::remove_var("HOME");
        clear_jcode_env();
        let _ = crate::integration::remove_dir_all_if_exists(&base);
    }

    /// FORK.md F1: `uninstall_jcode` removes only herdr's command and leaves a
    /// pre-existing dispatcher intact, and removes events herdr introduced
    /// outright rather than leaving them as empty tables. If this goes red,
    /// uninstalling jcode integration either strands a dead hook file or wipes
    /// a hook command the user configured themselves.
    #[test]
    fn fork_contract_uninstall_jcode_keeps_other_hook_commands_and_removes_only_herdrs() {
        let _lock = crate::integration::integration_env_lock();
        let base = unique_jcode_base();
        let jcode_dir = base.join("custom-jcode");
        std::fs::create_dir_all(&jcode_dir).unwrap();
        clear_jcode_env();
        std::env::set_var(crate::integration::JCODE_HOME_ENV_VAR, &jcode_dir);
        std::fs::write(
            jcode_dir.join("config.toml"),
            "[hooks]\nturn_end = \"echo keep\"\n",
        )
        .unwrap();

        crate::integration::install_jcode().unwrap();
        let result = crate::integration::uninstall_jcode().unwrap();
        let config = std::fs::read_to_string(&result.config_path).unwrap();

        assert!(result.removed_hook_file);
        assert!(result.updated_config);
        assert!(!result.hook_path.exists());
        assert_eq!(
            jcode_hook_commands(&config, "turn_end"),
            vec!["echo keep".to_string()]
        );
        assert!(jcode_hook_commands(&config, "post_tool").is_empty());
        assert!(jcode_hook_commands(&config, "session_start").is_empty());

        clear_jcode_env();
        let _ = crate::integration::remove_dir_all_if_exists(&base);
    }

    /// FORK.md F1: jcode's config editing must round-trip both TOML string
    /// forms a hand-written `config.toml` may use for a hook command (a
    /// literal string, or a multi-command array). If this goes red, a
    /// reinstall against a hand-edited config silently drops the user's hook.
    #[test]
    fn fork_contract_jcode_toml_values_round_trip_through_both_string_forms() {
        assert_eq!(
            crate::integration::parse_toml_string_or_array("'echo one'"),
            vec!["echo one".to_string()]
        );
        assert_eq!(
            crate::integration::parse_toml_string_or_array("[\"a\", 'b'] # trailing comment"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            crate::integration::parse_toml_string_or_array("\"quoted \\\" hash # not a comment\""),
            vec!["quoted \" hash # not a comment".to_string()]
        );
        assert_eq!(
            crate::integration::render_toml_string_or_array(&["one".to_string()]),
            "\"one\""
        );
        assert_eq!(
            crate::integration::render_toml_string_or_array(&[
                "one".to_string(),
                "two".to_string()
            ]),
            "[\"one\", \"two\"]"
        );
    }

    // -----------------------------------------------------------------
    // FORK.md F1 - jcode is a full lifecycle hook authority, deliberately
    // absent from SCREEN_MANIFEST_AGENTS
    // -----------------------------------------------------------------

    /// FORK.md F1: two authorities on one pane (screen detection AND a hook
    /// reporter) is the exact bug this gating avoids, so jcode must be a
    /// `full_lifecycle_hook_authority` pair AND absent from
    /// `Agent::SCREEN_MANIFEST_AGENTS`. If this goes red because jcode is back
    /// in the screen manifest, every jcode pane starts double-reporting state
    /// from two disagreeing sources.
    #[test]
    fn fork_contract_jcode_is_hook_authority_without_screen_manifest() {
        assert!(crate::detect::full_lifecycle_hook_authority(
            "herdr:jcode",
            "jcode"
        ));
        assert!(
            !crate::detect::Agent::SCREEN_MANIFEST_AGENTS.contains(&crate::detect::Agent::Jcode)
        );
        assert_eq!(
            crate::detect::identify_agent("jcode"),
            Some(crate::detect::Agent::Jcode)
        );
    }

    // -----------------------------------------------------------------
    // FORK.md F1 - a resumed jcode session re-anchors
    // -----------------------------------------------------------------

    /// FORK.md F1: `jcode --resume <id>` fires `session_start` twice, once as
    /// `new` for the throwaway session object the process starts with, then
    /// again as `resume` for the session actually restored. The second
    /// `session_start` (source `resume`) must replace the first anchor, and
    /// state reported against the new session id must apply. If this goes
    /// red, a resumed jcode pane gets stuck reporting against a session id
    /// nothing ever uses again, and every later state update is silently
    /// dropped.
    #[test]
    fn fork_contract_jcode_resume_reanchors_full_lifecycle_authority() {
        let mut terminal = crate::terminal::TerminalState::new(
            crate::terminal::TerminalId::alloc(),
            "/tmp".into(),
        );
        terminal.set_detected_state(
            Some(crate::detect::Agent::Jcode),
            crate::detect::AgentState::Idle,
        );
        let created = crate::agent_resume::AgentSessionRef::id("session_created").unwrap();
        let resumed = crate::agent_resume::AgentSessionRef::id("session_resumed").unwrap();

        assert!(
            terminal
                .set_agent_session_ref_for_session_start(
                    "herdr:jcode".into(),
                    "jcode".into(),
                    Some(created.clone()),
                    Some(10),
                    Some("new".into()),
                )
                .is_some(),
            "jcode should anchor the session it starts with"
        );
        assert!(terminal
            .set_hook_authority_with_session_ref(
                "herdr:jcode".into(),
                "jcode".into(),
                crate::detect::AgentState::Working,
                None,
                Some(created),
                Some(11),
            )
            .is_some());

        assert!(
            terminal
                .set_agent_session_ref_for_session_start(
                    "herdr:jcode".into(),
                    "jcode".into(),
                    Some(resumed.clone()),
                    Some(12),
                    Some("resume".into()),
                )
                .is_some(),
            "a resumed jcode session must replace the session it started with"
        );

        assert!(
            terminal
                .set_hook_authority_with_session_ref(
                    "herdr:jcode".into(),
                    "jcode".into(),
                    crate::detect::AgentState::Idle,
                    None,
                    Some(resumed),
                    Some(13),
                )
                .is_some(),
            "state from the resumed jcode session must apply"
        );
        assert_eq!(terminal.state, crate::detect::AgentState::Idle);
    }

    /// FORK.md F1: `herdr:jcode` is an official source, so a pane running
    /// jcode must restore as `jcode --resume <id>` after a server restart,
    /// not a bare shell, and an unofficial source reporting the same agent
    /// must be refused. Drives the same function
    /// (`crate::persist::restore_plan_for_snapshot`) the server calls on
    /// restart. Moved from `src/persist/restore.rs` (upstream-owned - a merge
    /// there could silently delete this hunk and take the feature's only
    /// guard with it); see the "moved to" comment left in its place.
    #[test]
    fn fork_contract_restore_plan_resumes_a_jcode_session() {
        use crate::persist::{restore_plan_for_snapshot, PaneAgentSessionSnapshot};

        let session = PaneAgentSessionSnapshot {
            source: "herdr:jcode".into(),
            agent: "jcode".into(),
            kind: crate::agent_resume::AgentSessionRefKind::Id,
            value: "session_goat_1787603691782_ec789f4bf85ebc01".into(),
        };

        assert!(restore_plan_for_snapshot(&session, false).is_none());
        assert_eq!(
            restore_plan_for_snapshot(&session, true).unwrap().argv,
            vec![
                "jcode",
                "--resume",
                "session_goat_1787603691782_ec789f4bf85ebc01"
            ]
        );

        // An unofficial source reporting the same agent must still be refused,
        // otherwise any process could make herdr launch a command on restart.
        let unofficial = PaneAgentSessionSnapshot {
            source: "custom:jcode".into(),
            agent: "jcode".into(),
            kind: crate::agent_resume::AgentSessionRefKind::Id,
            value: "session_goat_1787603691782_ec789f4bf85ebc01".into(),
        };
        assert!(restore_plan_for_snapshot(&unofficial, true).is_none());
    }

    // -----------------------------------------------------------------
    // FORK.md F2 - close_pane_if_idle
    // -----------------------------------------------------------------

    /// FORK.md F2: `close_pane_if_idle` is real client-process key-dispatch
    /// logic (`ClientShellState::close_focused_pane_if_idle`,
    /// `src/client/shell/input.rs`) that decides *before* a keystroke is
    /// classified onto the wire, so a raw socket standing in for the client
    /// (as `tests/fork_contract.rs` does for every other feature) cannot
    /// reach it - see that file's F2 section for why this lives here instead.
    /// This drives the real entry point the client's own input loop calls,
    /// `ClientShellState::handle_input_bytes`, with the literal Alt+X escape
    /// sequence a terminal sends, against a snapshot with and without an
    /// agent entry for the focused pane. If this goes red, either a user's
    /// live agent pane starts closing under them, or the binding stops
    /// closing anything at all.
    #[test]
    fn fork_contract_close_pane_if_idle_closes_an_agent_free_pane_but_not_one_with_an_agent() {
        use crate::client::shell::{ClientShellAction, ClientShellConfig, ClientShellState};
        use crate::protocol::{
            ClientShellAgent, ClientShellPane, ClientShellSnapshot, ClientShellTab,
            ClientShellWorkspace,
        };

        fn snapshot(with_agent: bool) -> ClientShellSnapshot {
            ClientShellSnapshot {
                boot_id: "boot-1".into(),
                revision: 1,
                config_diagnostic: None,
                product_announcement: None,
                update_available: None,
                update_install_command: "herdr update".into(),
                server_keybindings_toml: None,
                latest_release_notes_available: false,
                integration_updates_available: false,
                worktree_directory: "/tmp/herdr-worktrees".into(),
                release_notes: None,
                focused_workspace_id: Some("ws_1".into()),
                focused_tab_id: Some("tab_1".into()),
                focused_pane_id: Some("pane_1".into()),
                tab_bar_right: Vec::new(),
                tab_bar_right_separator: " ".into(),
                agent_view_label: None,
                agent_order: Vec::new(),
                workspaces: vec![ClientShellWorkspace {
                    workspace_id: "ws_1".into(),
                    active_tab_id: "tab_1".into(),
                    new_workspace_cwd: "/repo".into(),
                    number: 1,
                    label: "client-shell".into(),
                    custom_label: false,
                    branch: None,
                    git_ahead_behind: None,
                    tokens: Vec::new(),
                    worktree: None,
                    focused: true,
                    agent_status: crate::api::schema::AgentStatus::Idle,
                }],
                tabs: vec![ClientShellTab {
                    tab_id: "tab_1".into(),
                    workspace_id: "ws_1".into(),
                    number: 1,
                    label: "1".into(),
                    custom_label: false,
                    zoomed: false,
                    focused: true,
                    agent_status: crate::api::schema::AgentStatus::Idle,
                }],
                panes: vec![ClientShellPane {
                    pane_id: "pane_1".into(),
                    workspace_id: "ws_1".into(),
                    tab_id: "tab_1".into(),
                    label: None,
                    cwd: Some("/repo".into()),
                    foreground_cwd: Some("/repo".into()),
                    focused: true,
                    right_click_passthrough: false,
                }],
                agents: if with_agent {
                    vec![ClientShellAgent {
                        pane_id: "pane_1".into(),
                        workspace_id: "ws_1".into(),
                        tab_id: "tab_1".into(),
                        name: Some("fork-contract-agent".into()),
                        display_agent: None,
                        agent: Some("fork-contract-agent".into()),
                        title: None,
                        terminal_title: None,
                        terminal_title_stripped: None,
                        agent_status: crate::api::schema::AgentStatus::Working,
                        state_change_seq: 1,
                        state_labels: Vec::new(),
                        tokens: Vec::new(),
                        state_age_seconds: Some(1),
                        idle_age_seconds: None,
                        focused: true,
                    }]
                } else {
                    Vec::new()
                },
                commands: Vec::new(),
            }
        }

        fn requested_pane_close(actions: &[ClientShellAction]) -> bool {
            actions.iter().any(|action| {
                matches!(
                    action,
                    ClientShellAction::Endpoint { request, .. }
                        if matches!(request.method, crate::api::schema::Method::PaneClose(_))
                )
            })
        }

        let config: crate::config::Config =
            toml::from_str("[keys]\nclose_pane_if_idle = \"alt+x\"\n").expect("parse config");

        // Alt+X is ESC followed by 'x' - the literal bytes a terminal sends
        // for an Alt-modified key, the same encoding
        // `close_pane_if_idle_tests.rs`'s `alt_x()` helper builds by hand.
        const ALT_X_BYTES: &[u8] = &[0x1b, b'x'];

        // A pane with a reported agent must survive the chord: the key must
        // be forwarded, never turned into a close.
        let mut busy = ClientShellState::new(ClientShellConfig::from_config(&config));
        busy.set_snapshot(Box::new(snapshot(true)));
        let outcome = busy.handle_input_bytes(ALT_X_BYTES);
        assert!(
            !requested_pane_close(&outcome.actions),
            "a pane with a reported agent must not close on close_pane_if_idle: {:?}",
            outcome.actions
        );

        // The same chord against an agent-free pane must request the close.
        let mut idle = ClientShellState::new(ClientShellConfig::from_config(&config));
        idle.set_snapshot(Box::new(snapshot(false)));
        let outcome = idle.handle_input_bytes(ALT_X_BYTES);
        assert!(
            requested_pane_close(&outcome.actions),
            "close_pane_if_idle should close an agent-free pane: {:?}",
            outcome.actions
        );
    }

    /// FORK.md F4: `state_entered_at` must advance only on a real state
    /// transition, never on a repeated report of the same state, because
    /// screen detection re-reports the same state on every scan. If this goes
    /// red, the idle-staleness clock restarts on every detection tick and
    /// nothing ever crosses the stale threshold.
    #[test]
    fn fork_contract_state_entered_at_tracks_only_real_state_changes() {
        let mut terminal = crate::terminal::TerminalState::new(
            crate::terminal::TerminalId::alloc(),
            "/tmp".into(),
        );
        let t0 = Instant::now();

        terminal.set_detected_state_with_screen_signals_at(
            Some(crate::detect::Agent::Pi),
            crate::detect::AgentState::Idle,
            false,
            false,
            false,
            false,
            t0,
        );
        assert_eq!(terminal.state, crate::detect::AgentState::Idle);
        assert_eq!(terminal.state_entered_at(), t0);

        let later = t0 + Duration::from_secs(60);
        terminal.set_detected_state_with_screen_signals_at(
            Some(crate::detect::Agent::Pi),
            crate::detect::AgentState::Idle,
            false,
            false,
            false,
            false,
            later,
        );
        assert_eq!(
            terminal.state_entered_at(),
            t0,
            "a repeated report of the same state must not restart the clock"
        );

        let moved = t0 + Duration::from_secs(90);
        terminal.set_detected_state_with_screen_signals_at(
            Some(crate::detect::Agent::Pi),
            crate::detect::AgentState::Working,
            false,
            false,
            false,
            false,
            moved,
        );
        assert_eq!(terminal.state, crate::detect::AgentState::Working);
        assert_eq!(terminal.state_entered_at(), moved);
    }

    /// FORK.md F4: `mark_seen` must advance the look clock every time it is
    /// called, including a re-focus of an already-seen pane, or a pane the
    /// user keeps checking would still age into the parked bucket. If this
    /// goes red, glancing at a pane repeatedly stops resetting its "last
    /// looked at" time.
    #[test]
    fn fork_contract_mark_seen_advances_the_look_clock_every_time() {
        let mut pane = crate::pane::PaneState::new(crate::terminal::TerminalId::alloc());
        let t0 = Instant::now();

        pane.seen = false;
        pane.seen_at = t0;
        assert!(pane.mark_seen(t0 + Duration::from_secs(10)));
        assert!(pane.seen);
        assert_eq!(pane.seen_at, t0 + Duration::from_secs(10));

        assert!(!pane.mark_seen(t0 + Duration::from_secs(20)));
        assert_eq!(pane.seen_at, t0 + Duration::from_secs(20));
    }

    /// FORK.md F4: an unread result crossing the stale threshold raises the
    /// needs-attention notification exactly once per idle episode, and a pane
    /// the user has looked at never alerts on going parked (parking is
    /// deliberate). If this goes red, either a user gets spammed every timer
    /// tick for one stale pane, or a parked pane starts alerting.
    #[test]
    fn fork_contract_unread_result_alerts_once_when_stale_but_never_when_looked_at() {
        let mut state = crate::app::AppState::test_new();
        state
            .workspaces
            .push(crate::workspace::Workspace::test_new("a"));
        state.ensure_test_terminals();
        state.active = Some(0);
        state.idle_stale_after = Duration::from_secs(300);
        let pane_id = *state.workspaces[0].panes.keys().next().unwrap();
        let terminal_id = state.workspaces[0].panes[&pane_id]
            .attached_terminal_id
            .clone();

        let t0 = Instant::now();
        state
            .terminals
            .get_mut(&terminal_id)
            .expect("terminal")
            .set_detected_state_with_screen_signals_at(
                Some(crate::detect::Agent::Pi),
                crate::detect::AgentState::Idle,
                false,
                false,
                false,
                false,
                t0,
            );

        // Unseen and off the active tab, which is what "you have not looked" is.
        state.active = None;
        state.workspaces[0]
            .panes
            .get_mut(&pane_id)
            .expect("pane")
            .seen = false;

        let crossed = t0 + Duration::from_secs(300);
        let first = state.alert_stale_unread_panes_at(crossed);
        assert_eq!(first.len(), 1, "crossing the threshold must alert once");

        let later = crossed + Duration::from_secs(60);
        assert!(
            state.alert_stale_unread_panes_at(later).is_empty(),
            "the same idle episode must not alert twice"
        );

        // A pane the user has looked at must never alert on going parked.
        state.workspaces[0]
            .panes
            .get_mut(&pane_id)
            .expect("pane")
            .mark_seen(later);
        let parked = later + Duration::from_secs(300);
        assert!(
            state.alert_stale_unread_panes_at(parked).is_empty(),
            "parking is deliberate and must stay silent"
        );
    }

    // -----------------------------------------------------------------
    // FORK.md F4 - idle aging visible: the four buckets render distinct
    // glyphs and labels in the drawn agent row
    // -----------------------------------------------------------------

    /// FORK.md F4: the four idle buckets (FreshUnseen/StaleUnseen/FreshSeen/
    /// ParkedSeen) must render distinct glyphs, driven by the age that
    /// arrives on the client snapshot's `state_age_seconds` - not by calling
    /// the classification function directly, but by reading the actual drawn
    /// row from `agent_sidebar::render_agent_row`. If this goes red, either
    /// the age never reached the client snapshot again, or the draw site
    /// stopped consulting it, and every idle pane looks identical regardless
    /// of how long it has been sitting.
    #[test]
    fn fork_contract_idle_age_glyph_renders_from_wire_state_age_seconds() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let mut config = crate::config::Config::default();
        config.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
        config.ui.idle_stale_after_seconds = 300;
        let shell_config = crate::client::shell::ClientShellConfig::from_config(&config);

        let icon_for = |status: crate::api::schema::AgentStatus, idle_age_seconds: u64| {
            let row = crate::client::shell::AgentRow {
                pane_id: "pane_1".into(),
                status,
                focused: false,
                rows: Vec::new(),
                nested: false,
                last_child: false,
                idle_age_seconds: Some(idle_age_seconds),
            };
            let rect = Rect::new(0, 0, 10, 1);
            let mut buffer = Buffer::empty(rect);
            crate::client::shell::render_agent_row(&mut buffer, rect, &row, &shell_config);
            // Indent for a top-level, non-nested row is one space, so the icon
            // is the second cell.
            buffer.cell((1, 0)).expect("icon cell").symbol().to_owned()
        };

        use crate::api::schema::AgentStatus;
        assert_eq!(
            icon_for(AgentStatus::Done, 10),
            "✓",
            "fresh unseen (done) should keep its glyph"
        );
        assert_eq!(
            icon_for(AgentStatus::Done, 3_600),
            "!",
            "stale unseen should get the distinct stale glyph"
        );
        assert_eq!(
            icon_for(AgentStatus::Idle, 10),
            "○",
            "fresh seen (idle) should keep its glyph"
        );
        assert_eq!(
            icon_for(AgentStatus::Idle, 3_600),
            "◌",
            "parked seen should get the distinct parked glyph"
        );
    }

    /// FORK.md F4, review must-fix #1: a *seen* pane must age from the look
    /// clock (`pane.seen_at`), not the state clock
    /// (`terminal.state_entered_at()`), the same two-clock rule
    /// `crate::workspace::pane_aged_from` already uses for the server-side
    /// stale/parked alert. Drives a real `App`: a real hook report (the same
    /// `AppEvent::HookStateReported` the API layer emits for
    /// `pane.report_agent`), a real sleep so the state clock actually ages,
    /// then a real look (`focus_pane_in_workspace` + `mark_active_tab_seen`,
    /// the same two calls `pane.focus` makes) - not a hand-set age on a
    /// synthetic row. If this goes red, a pane the user just looked at draws
    /// the parked glyph instead of idle, disagreeing with the server-side
    /// alert about the same pane.
    #[test]
    fn fork_contract_pane_reports_idle_age_seconds_from_the_look_clock() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state
            .workspaces
            .push(crate::workspace::Workspace::test_new("fork-idle-age"));
        app.state.ensure_test_terminals();

        let pane_id = *app.state.workspaces[0]
            .panes
            .keys()
            .next()
            .expect("a fresh workspace has a pane");

        // No workspace active yet, i.e. "you have not looked": otherwise the
        // active-tab heuristic treats the incoming report as already seen.
        app.state.active = None;

        // Real report: exactly the event `handle_pane_report_agent` emits
        // for a live `pane.report_agent` call. Working then idle is a real
        // completion transition (`is_background_completion_transition`),
        // which is what actually clears `pane.seen` - reporting idle alone
        // from the pane's default `seen: true` would prove nothing.
        app.handle_internal_event(crate::events::AppEvent::HookStateReported {
            pane_id,
            source: "herdr:fork-contract-test".into(),
            agent_label: "fork-contract-agent".into(),
            state: crate::detect::AgentState::Working,
            message: None,
            seq: None,
            session_ref: None,
        });
        app.handle_internal_event(crate::events::AppEvent::HookStateReported {
            pane_id,
            source: "herdr:fork-contract-test".into(),
            agent_label: "fork-contract-agent".into(),
            state: crate::detect::AgentState::Idle,
            message: None,
            seq: None,
            session_ref: None,
        });

        std::thread::sleep(Duration::from_millis(1200));

        let unseen = app.agent_info(0, pane_id).expect("agent info exists");
        assert_eq!(
            unseen.agent_status,
            crate::api::schema::AgentStatus::Done,
            "a freshly reported idle pane nobody has looked at is unseen"
        );
        let unseen_age = unseen
            .idle_age_seconds
            .expect("idle_age_seconds must be reported once an agent state exists");
        assert!(
            unseen_age >= 1,
            "an unseen pane must age from the result clock: got {unseen_age}"
        );

        // Real look: the exact state mutation `pane.focus` performs.
        app.state.focus_pane_in_workspace(0, pane_id);
        app.state.mark_active_tab_seen();

        let seen = app.agent_info(0, pane_id).expect("agent info exists");
        assert_eq!(
            seen.agent_status,
            crate::api::schema::AgentStatus::Idle,
            "focusing the pane must mark it seen"
        );
        let seen_age = seen
            .idle_age_seconds
            .expect("idle_age_seconds must be reported once an agent state exists");
        assert_eq!(
            seen_age, 0,
            "a pane just looked at must age from the look clock, not the stale result clock: got {seen_age}"
        );

        // The drawn glyph must actually differ, fed the real ages above, not
        // hand-set numbers: the unseen pane is stale, the just-looked-at
        // pane is fresh.
        let mut config = crate::config::Config::default();
        config.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
        config.ui.idle_stale_after_seconds = 1;
        let shell_config = crate::client::shell::ClientShellConfig::from_config(&config);

        let icon_for = |status: crate::api::schema::AgentStatus, idle_age_seconds: Option<u64>| {
            let row = crate::client::shell::AgentRow {
                pane_id: "pane_1".into(),
                status,
                focused: false,
                rows: Vec::new(),
                nested: false,
                last_child: false,
                idle_age_seconds,
            };
            let rect = ratatui::layout::Rect::new(0, 0, 10, 1);
            let mut buffer = ratatui::buffer::Buffer::empty(rect);
            crate::client::shell::render_agent_row(&mut buffer, rect, &row, &shell_config);
            buffer.cell((1, 0)).expect("icon cell").symbol().to_owned()
        };

        let unseen_glyph = icon_for(unseen.agent_status, unseen.idle_age_seconds);
        let seen_glyph = icon_for(seen.agent_status, seen.idle_age_seconds);
        assert_eq!(
            unseen_glyph, "!",
            "the unseen stale pane must draw the stale glyph"
        );
        assert_eq!(
            seen_glyph, "○",
            "the just-looked-at pane must draw the fresh glyph, not the parked glyph the bug drew before the fix"
        );
        assert_ne!(
            unseen_glyph, seen_glyph,
            "an unseen stale pane and a seen-then-aged pane must draw different glyphs"
        );
    }

    // -----------------------------------------------------------------
    // FORK.md F5 - agents above spaces, subagent nesting
    // -----------------------------------------------------------------

    /// FORK.md F5: a pane that reports `parent_pane` renders indented under
    /// that pane in the drawn agent rows, and a missing or cyclic parent
    /// degrades to a flat row rather than a broken one. This drives the real
    /// composed frame (`ClientShellState::compose`), not the ordering
    /// function alone, because this exact feature was computed correctly but
    /// rendered flat for weeks after the v0.9.0 port - the failure mode this
    /// port exists to catch. If this goes red, subagent panes scatter as
    /// siblings again.
    #[test]
    fn fork_contract_subagent_panes_render_indented_under_their_parent() {
        use crate::protocol::{
            ClientShellAgent, ClientShellPane, ClientShellSnapshot, ClientShellTab,
            ClientShellWorkspace,
        };

        let mut snapshot = ClientShellSnapshot {
            boot_id: "boot-1".into(),
            revision: 1,
            config_diagnostic: None,
            product_announcement: None,
            update_available: None,
            update_install_command: "herdr update".into(),
            server_keybindings_toml: None,
            latest_release_notes_available: false,
            integration_updates_available: false,
            worktree_directory: "/tmp/herdr-worktrees".into(),
            release_notes: None,
            focused_workspace_id: Some("ws_1".into()),
            focused_tab_id: Some("tab_1".into()),
            focused_pane_id: Some("pane_1".into()),
            tab_bar_right: Vec::new(),
            tab_bar_right_separator: " ".into(),
            agent_view_label: None,
            agent_order: Vec::new(),
            workspaces: vec![ClientShellWorkspace {
                workspace_id: "ws_1".into(),
                active_tab_id: "tab_1".into(),
                new_workspace_cwd: "/repo".into(),
                number: 1,
                label: "client-shell".into(),
                custom_label: false,
                branch: Some("main".into()),
                git_ahead_behind: None,
                tokens: Vec::new(),
                worktree: None,
                focused: true,
                agent_status: crate::api::schema::AgentStatus::Idle,
            }],
            tabs: vec![ClientShellTab {
                tab_id: "tab_1".into(),
                workspace_id: "ws_1".into(),
                number: 1,
                label: "1".into(),
                custom_label: false,
                zoomed: false,
                focused: true,
                agent_status: crate::api::schema::AgentStatus::Idle,
            }],
            panes: vec![ClientShellPane {
                pane_id: "pane_1".into(),
                workspace_id: "ws_1".into(),
                tab_id: "tab_1".into(),
                label: None,
                cwd: Some("/repo".into()),
                foreground_cwd: Some("/repo".into()),
                focused: true,
                right_click_passthrough: false,
            }],
            agents: Vec::new(),
            commands: Vec::new(),
        };
        for pane_id in ["pane_2", "pane_3"] {
            let mut pane = snapshot.panes[0].clone();
            pane.pane_id = pane_id.into();
            pane.focused = false;
            snapshot.panes.push(pane);
        }
        let agent = |pane_id: &str, name: &str, seq: u64, parent: Option<&str>| ClientShellAgent {
            pane_id: pane_id.into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some(name.into()),
            display_agent: None,
            agent: Some("pi".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: crate::api::schema::AgentStatus::Idle,
            state_change_seq: seq,
            state_labels: Vec::new(),
            tokens: parent
                .map(|parent| vec![("parent_pane".into(), parent.into())])
                .unwrap_or_default(),
            state_age_seconds: None,
            idle_age_seconds: None,
            focused: pane_id == "pane_1",
        };
        // The children sort ahead of the parent on their own, so a passing
        // run proves the nesting pass reordered them rather than the sort
        // agreeing by coincidence.
        snapshot.agents = vec![
            agent("pane_3", "child-b", 50, Some("pane_1")),
            agent("pane_1", "parent", 30, None),
            agent("pane_2", "child-a", 40, Some("pane_1")),
        ];

        let mut config = crate::config::Config::default();
        config.ui.agent_panel_sort = crate::config::AgentPanelSortConfig::Priority;
        config.ui.sidebar.agents.rows = vec![vec![crate::config::AgentSidebarToken::Agent]];
        let mut state = crate::client::shell::ClientShellState::new(
            crate::client::shell::ClientShellConfig::from_config(&config),
        );
        state.set_snapshot(Box::new(snapshot));

        let surface_buffer = ratatui::buffer::Buffer::with_lines(["LIVE", "PANE"]);
        state.set_pane_surface(crate::protocol::PaneSurfaceFrame {
            boot_id: "boot-1".into(),
            projection_revision: 1,
            surface_revision: 1,
            frame: crate::protocol::FrameData::from_ratatui_buffer_with_hyperlinks(
                &surface_buffer,
                Some(crate::protocol::CursorState {
                    x: 1,
                    y: 1,
                    visible: true,
                    shape: 2,
                }),
                &[],
            ),
            panes: vec![crate::protocol::PaneSurfacePane {
                pane_id: "pane_1".into(),
                content_revision: 0,
                rect: crate::protocol::SurfaceRect {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 2,
                },
                inner_rect: crate::protocol::SurfaceRect {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 2,
                },
                scrollbar_rect: None,
                scroll: None,
                focused: true,
                mouse_reporting: false,
                sgr_pixel_mouse: false,
                alternate_screen_active: false,
                pixel_width: 0,
                pixel_height: 0,
            }],
            splits: Vec::new(),
            popup: None,
            graphics: crate::protocol::SurfaceGraphicsScene::default(),
        });

        let frame = state.compose(106, 30).expect("agent sidebar frame");
        let lines = frame
            .cells
            .chunks(frame.width as usize)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let row_of = |needle: &str| {
            lines
                .iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle} missing from {lines:#?}"))
        };
        let parent = row_of("parent");
        assert_eq!(row_of("├─ child-b"), parent + 1, "{lines:#?}");
        assert_eq!(row_of("└─ child-a"), parent + 2, "{lines:#?}");
        assert!(lines[parent + 1].starts_with("   ├─ "), "{lines:#?}");
    }
}
