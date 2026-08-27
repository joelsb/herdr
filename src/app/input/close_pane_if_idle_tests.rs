// `close_pane_if_idle`: one chord that closes the focused pane only when
// nothing is running in it, and otherwise forwards the key to the program that
// owns the pane.
//
// The motivating workflow is a single chord (Alt+X) that first quits the agent
// running in the pane, then closes the pane the agent left behind. That only
// works if the chord is *not* consumed while the agent is still running, so the
// passthrough tests below are the load-bearing ones, not the close test.

use super::*;

fn app_with_focused_pane() -> (App, crate::layout::PaneId, crate::terminal::TerminalId) {
    let mut app = App::new(
        &crate::config::Config::default(),
        true,
        None,
        tokio::sync::mpsc::unbounded_channel().1,
        crate::api::EventHub::default(),
    );
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    let terminal_id = ws.terminal_id(pane_id).cloned().expect("terminal id");
    ws.tabs[0].runtimes.insert(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, &[]),
    );
    app.state.terminals.insert(
        terminal_id.clone(),
        crate::terminal::TerminalState::new(terminal_id.clone(), "/tmp".into()),
    );
    app.state.workspaces = vec![ws];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;
    (app, pane_id, terminal_id)
}

fn alt_x() -> TerminalKey {
    TerminalKey::new(KeyCode::Char('x'), KeyModifiers::ALT)
}

/// Unbound by default: the action closes a pane on a bare chord, so it must be
/// an explicit opt-in rather than something a user discovers by accident.
#[test]
fn close_pane_if_idle_is_unbound_by_default() {
    let kb = crate::config::Config::default().keybinds();
    assert!(
        kb.close_pane_if_idle.bindings.is_empty(),
        "close_pane_if_idle must be opt-in"
    );
}

/// A bare `alt+x` is accepted rather than rejected as an unsafe direct binding:
/// it carries a modifier, so it cannot swallow ordinary typing.
#[test]
fn close_pane_if_idle_accepts_a_bare_alt_chord_from_config() {
    let config: crate::config::Config = toml::from_str(
        r#"
[keys]
close_pane_if_idle = "alt+x"
"#,
    )
    .expect("parse config");
    let diagnostics = config.collect_diagnostics();
    let kb = config.keybinds();

    assert!(
        kb.close_pane_if_idle.matches_direct_key(&alt_x()),
        "alt+x should bind directly, diagnostics: {diagnostics:?}"
    );
}

/// The key must not be consumed when it is not bound at all, or every session
/// without the binding would lose Alt+X in its panes.
#[tokio::test]
async fn unbound_chord_is_not_consumed() {
    let (mut app, _pane, _terminal) = app_with_focused_pane();
    assert!(
        !app.close_focused_pane_if_idle_requested(&alt_x()),
        "an unbound chord must fall through to the pane"
    );
    assert_eq!(app.state.workspaces.len(), 1, "nothing should have closed");
}

/// The load-bearing case. A pane running an agent must receive the chord itself,
/// so the agent can act on it (jcode exits on Alt+X). Consuming it here would
/// make the chord close the pane out from under a running agent.
#[tokio::test]
async fn chord_passes_through_to_a_pane_hosting_an_agent() {
    let (mut app, _pane, terminal_id) = app_with_focused_pane();
    app.state.keybinds.close_pane_if_idle = crate::config::ActionKeybinds::direct("alt+x");

    // Mark the pane as hosting an agent, which is herdr's own "this pane is
    // busy" signal, the same one agent.start refuses to overwrite.
    app.state
        .terminals
        .get_mut(&terminal_id)
        .expect("terminal")
        .agent_name = Some("jcode".into());

    assert!(
        !app.close_focused_pane_if_idle_requested(&alt_x()),
        "a pane hosting an agent must keep receiving the chord"
    );
    assert_eq!(
        app.state.workspaces.len(),
        1,
        "the pane must not be closed while an agent owns it"
    );
}

/// Fail-closed: with no focused pane there is nothing to prove idle, so the
/// chord is not consumed and certainly does not close anything.
#[test]
fn chord_is_not_consumed_without_a_focused_pane() {
    let mut app = App::new(
        &crate::config::Config::default(),
        true,
        None,
        tokio::sync::mpsc::unbounded_channel().1,
        crate::api::EventHub::default(),
    );
    app.state.keybinds.close_pane_if_idle = crate::config::ActionKeybinds::direct("alt+x");

    assert!(
        !app.close_focused_pane_if_idle_requested(&alt_x()),
        "no focused pane means nothing to close"
    );
}

/// A different chord must never trigger the action, even when the pane is idle.
#[tokio::test]
async fn a_different_chord_does_not_close_an_idle_pane() {
    let (mut app, _pane, _terminal) = app_with_focused_pane();
    app.state.keybinds.close_pane_if_idle = crate::config::ActionKeybinds::direct("alt+x");

    let other = TerminalKey::new(KeyCode::Char('y'), KeyModifiers::ALT);
    assert!(
        !app.close_focused_pane_if_idle_requested(&other),
        "only the bound chord may close the pane"
    );
    assert_eq!(app.state.workspaces.len(), 1);
}

/// An idle pane is closed. In this harness the runtime has no child PID, which
/// `available_shell_name` treats as a bare shell, so this exercises the
/// consume-and-close path.
#[tokio::test]
async fn idle_pane_is_closed_and_the_chord_is_consumed() {
    let (mut app, _pane, _terminal) = app_with_focused_pane();
    app.state.keybinds.close_pane_if_idle = crate::config::ActionKeybinds::direct("alt+x");

    assert!(
        app.close_focused_pane_if_idle_requested(&alt_x()),
        "an idle pane must consume the chord and close"
    );
}
