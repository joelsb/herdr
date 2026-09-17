// `close_pane_if_idle`: one chord that closes the focused pane only when
// nothing is running in it, and otherwise forwards the key to the program that
// owns the pane.
//
// The motivating workflow is a single chord (Alt+X) that first quits the agent
// running in the pane, then closes the pane the agent left behind. That only
// works if the chord is *not* consumed while the agent is still running.
//
// Ported to the server/client split (v0.9.0): dispatch and the idle check now
// live client-side (`ClientShellState::close_focused_pane_if_idle` in
// `src/client/shell/input.rs`), driven by the cached endpoint snapshot rather
// than a direct process-tree check, so only config-parsing behavior is
// exercised here. See `docs/next/CHANGELOG.md` / the port notes for the
// fidelity gap this leaves (a non-agent foreground process, e.g. `vim` in an
// unlabeled pane, is not distinguished from a bare shell).

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

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
