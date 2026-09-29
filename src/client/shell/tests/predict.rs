//! Predictive local echo (FORK.md F8). See `src/client/shell/predict.rs` for
//! the implementation this drives.
use super::*;
use crate::client::endpoint::{
    ClientEndpointId, ClientEndpointStatus, ProfileId, SavedSshEndpoint,
};

fn remote_profile() -> SavedSshEndpoint {
    SavedSshEndpoint {
        id: ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    }
}

/// A pane surface with a blank row so a fresh guess run is always eligible
/// at (2, 0), regardless of `surface()`'s own (non-blank-tailed) content.
fn blank_remote_surface() -> PaneSurfaceFrame {
    let buffer = Buffer::with_lines(["          ", "          "]);
    PaneSurfaceFrame {
        boot_id: "boot-1".into(),
        projection_revision: 1,
        surface_revision: 1,
        frame: FrameData::from_ratatui_buffer_with_hyperlinks(
            &buffer,
            Some(crate::protocol::CursorState {
                x: 2,
                y: 0,
                visible: true,
                shape: 2,
            }),
            &[],
        ),
        panes: vec![PaneSurfacePane {
            pane_id: "pane_1".into(),
            content_revision: 0,
            rect: SurfaceRect {
                x: 0,
                y: 0,
                width: 10,
                height: 2,
            },
            inner_rect: SurfaceRect {
                x: 0,
                y: 0,
                width: 10,
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
    }
}

/// Same shape as `blank_remote_surface()`, but with `ch` already sitting at
/// (2, 0) - as if the server just echoed it back - for confirm/mismatch
/// tests. `surface_revision` bumps so `set_pane_surface` accepts it as a
/// newer frame for the same pane surface pair.
fn remote_surface_with_char_at_cursor(ch: char, surface_revision: u64) -> PaneSurfaceFrame {
    let mut line = "          ".to_string();
    line.replace_range(2..3, &ch.to_string());
    let buffer = Buffer::with_lines([line.as_str(), "          "]);
    let mut surface = blank_remote_surface();
    surface.surface_revision = surface_revision;
    surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(
        &buffer,
        Some(crate::protocol::CursorState {
            x: 3,
            y: 0,
            visible: true,
            shape: 2,
        }),
        &[],
    );
    surface
}

fn key_event(ch: char) -> crate::protocol::ClientPaneInputEvent {
    crate::protocol::ClientPaneInputEvent::Key {
        code: crate::protocol::ClientKeyCode::Char(ch),
        modifiers: 0,
        kind: crate::protocol::ClientKeyKind::Press,
        repeat_count: 1,
        shifted_codepoint: None,
        generated_text: None,
        tracks_release: false,
        physical_key_id: None,
        windows_record: None,
    }
}

fn control_key_event(
    code: crate::protocol::ClientKeyCode,
) -> crate::protocol::ClientPaneInputEvent {
    crate::protocol::ClientPaneInputEvent::Key {
        code,
        modifiers: 0,
        kind: crate::protocol::ClientKeyKind::Press,
        repeat_count: 1,
        shifted_codepoint: None,
        generated_text: None,
        tracks_release: false,
        physical_key_id: None,
        windows_record: None,
    }
}

fn remote_state_with_surface() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let profile = remote_profile();
    let endpoint_id = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&endpoint_id, ClientEndpointStatus::Online);
    state.set_endpoint_snapshot(&endpoint_id, Box::new(snapshot()));
    assert!(state.activate_endpoint_projection(&endpoint_id));
    state.set_pane_surface(blank_remote_surface());
    state
}

/// Behaviour #4: a pane on the local endpoint never predicts, even with the
/// same eligible cursor/blank-row shape a remote pane would guess from.
#[test]
fn local_endpoint_pane_never_predicts() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(blank_remote_surface());
    state.record_pane_prediction("pane_1", &key_event('a'));
    assert!(state.pane_predictions.is_empty());
}

/// Behaviour #1: a printable key in a remote pane, once the epoch is
/// already confirmed, draws a dim cell with that character at the cursor
/// before any frame arrives to confirm it.
#[test]
fn printable_key_in_confirmed_remote_pane_draws_dim_before_any_frame() {
    let mut state = remote_state_with_surface();
    state.pane_predictions.insert(
        "pane_1".to_string(),
        super::super::predict::PanePrediction {
            epoch: super::super::predict::PredictionEpoch::Confirmed,
            guesses: Vec::new(),
            resume_at: Some((2, 0)),
        },
    );

    state.record_pane_prediction("pane_1", &key_event('x'));

    let area = state.layout(40, 12).pane_surface;
    let composed = state.compose(40, 12).expect("composed frame");
    let buffer = composed.to_ratatui_buffer().expect("ratatui buffer");
    let cell = &buffer[(area.x + 2, area.y)];
    assert_eq!(cell.symbol(), "x");
    assert!(cell.modifier.contains(Modifier::DIM));
}

/// Behaviour #2: once the server's frame shows the guessed character at the
/// predicted cell, the overlay stops drawing it dim (real content shows
/// through) and the guess is dropped from tracking.
#[test]
fn server_frame_confirming_guess_drops_it_and_stops_drawing_dim() {
    let mut state = remote_state_with_surface();
    state.pane_predictions.insert(
        "pane_1".to_string(),
        super::super::predict::PanePrediction {
            epoch: super::super::predict::PredictionEpoch::Confirmed,
            guesses: Vec::new(),
            resume_at: Some((2, 0)),
        },
    );
    state.record_pane_prediction("pane_1", &key_event('x'));
    assert_eq!(
        state.pane_predictions.get("pane_1").unwrap().guesses.len(),
        1,
        "precondition: one pending guess before the confirming frame"
    );

    state.set_pane_surface(remote_surface_with_char_at_cursor('x', 2));
    let area = state.layout(40, 12).pane_surface;
    let composed = state.compose(40, 12).expect("composed frame");
    let buffer = composed.to_ratatui_buffer().expect("ratatui buffer");
    let cell = &buffer[(area.x + 2, area.y)];

    assert_eq!(cell.symbol(), "x");
    assert!(!cell.modifier.contains(Modifier::DIM));
    assert!(
        state
            .pane_predictions
            .get("pane_1")
            .is_none_or(|prediction| prediction.guesses.is_empty()),
        "confirmed guess must be dropped from tracking"
    );
}

/// Behaviour #3: after Enter, the epoch resets to unconfirmed. Typing into
/// what is now a password prompt (no echo ever arrives) never draws
/// anything, at any point, for any of the typed characters.
#[test]
fn password_prompt_after_enter_never_draws_and_pauses() {
    let mut state = remote_state_with_surface();
    state.pane_predictions.insert(
        "pane_1".to_string(),
        super::super::predict::PanePrediction {
            epoch: super::super::predict::PredictionEpoch::Confirmed,
            guesses: Vec::new(),
            resume_at: Some((2, 0)),
        },
    );

    state.record_pane_prediction(
        "pane_1",
        &control_key_event(crate::protocol::ClientKeyCode::Enter),
    );
    assert!(
        state.pane_predictions.is_empty(),
        "Enter must clear guesses and reset the epoch to unconfirmed"
    );

    for ch in ['s', 'w', 'o', 'r'] {
        state.record_pane_prediction("pane_1", &key_event(ch));
        let area = state.layout(40, 12).pane_surface;
        let composed = state.compose(40, 12).expect("composed frame");
        let buffer = composed.to_ratatui_buffer().expect("ratatui buffer");
        for x in 0..10u16 {
            let cell = &buffer[(area.x + x, area.y)];
            assert_eq!(
                cell.symbol().trim(),
                "",
                "no character may ever be drawn while the epoch is unconfirmed (password safety)"
            );
        }
    }
}

/// Wiring check: the real input dispatch path (`handle_raw_events` ->
/// `handle_key` -> `push_pane_key` -> `record_pane_prediction`) reaches
/// predictive echo, not just a direct call to an internal method. A future
/// merge that drops the one-line hook in `input.rs` would leave every other
/// test in this file passing (they all call `record_pane_prediction` or
/// seed `pane_predictions` directly) while the real feature went silently
/// dead - this is the test that would catch it.
#[test]
fn real_input_dispatch_reaches_predictive_echo() {
    let mut state = remote_state_with_surface();
    state.pane_predictions.insert(
        "pane_1".to_string(),
        super::super::predict::PanePrediction {
            epoch: super::super::predict::PredictionEpoch::Confirmed,
            guesses: Vec::new(),
            resume_at: Some((2, 0)),
        },
    );

    let key = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Char('x'),
        crossterm::event::KeyModifiers::empty(),
    );
    state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Key(key)]);

    let area = state.layout(40, 12).pane_surface;
    let composed = state.compose(40, 12).expect("composed frame");
    let buffer = composed.to_ratatui_buffer().expect("ratatui buffer");
    let cell = &buffer[(area.x + 2, area.y)];
    assert_eq!(cell.symbol(), "x");
    assert!(cell.modifier.contains(Modifier::DIM));
}

/// Behaviour #5: Enter and an arrow both clear pending guesses (Enter is
/// covered above; this covers the "any of these clears" catch-all with a
/// deliberately reverted control to prove the assertion is load-bearing -
/// see the DONE line for the RED observed on this test with the catch-all
/// arm commented out).
#[test]
fn enter_and_arrow_clear_pending_guesses() {
    let mut state = remote_state_with_surface();
    state.pane_predictions.insert(
        "pane_1".to_string(),
        super::super::predict::PanePrediction {
            epoch: super::super::predict::PredictionEpoch::Confirmed,
            guesses: vec![super::super::predict::PredictedGuess {
                x: 2,
                y: 0,
                ch: 'x',
                requested_at: std::time::Instant::now(),
            }],
            resume_at: Some((2, 0)),
        },
    );

    state.record_pane_prediction(
        "pane_1",
        &control_key_event(crate::protocol::ClientKeyCode::Left),
    );

    assert!(
        state.pane_predictions.is_empty(),
        "an arrow key must clear all guesses for the pane"
    );
}

/// Behaviour #6: an unconfirmed guess older than 250ms is dropped, even
/// with no other input, so a stale dim character never lingers forever.
#[test]
fn unconfirmed_guess_older_than_250ms_is_dropped() {
    let mut state = remote_state_with_surface();
    let stale = std::time::Instant::now() - std::time::Duration::from_millis(300);
    state.pane_predictions.insert(
        "pane_1".to_string(),
        super::super::predict::PanePrediction {
            epoch: super::super::predict::PredictionEpoch::Confirmed,
            guesses: vec![super::super::predict::PredictedGuess {
                x: 2,
                y: 0,
                ch: 'x',
                requested_at: stale,
            }],
            resume_at: Some((2, 0)),
        },
    );

    let changed = state.tick_predictive_echo(std::time::Instant::now());

    assert!(changed, "tick must report that a guess was dropped");
    assert!(!state.pane_predictions.contains_key("pane_1"));

    let area = state.layout(40, 12).pane_surface;
    let composed = state.compose(40, 12).expect("composed frame");
    let buffer = composed.to_ratatui_buffer().expect("ratatui buffer");
    let cell = &buffer[(area.x + 2, area.y)];
    assert_eq!(cell.symbol().trim(), "");
}

/// A surface whose cursor sits at `(x, y)` on otherwise blank rows, as if the
/// program just printed a new prompt somewhere else on screen.
fn remote_surface_with_cursor_at(x: u16, y: u16, surface_revision: u64) -> PaneSurfaceFrame {
    let buffer = Buffer::with_lines(["          ", "          "]);
    let mut surface = blank_remote_surface();
    surface.surface_revision = surface_revision;
    surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(
        &buffer,
        Some(crate::protocol::CursorState {
            x,
            y,
            visible: true,
            shape: 2,
        }),
        &[],
    );
    surface
}

fn confirmed_at(x: u16, y: u16) -> super::super::predict::PanePrediction {
    super::super::predict::PanePrediction {
        epoch: super::super::predict::PredictionEpoch::Confirmed,
        guesses: Vec::new(),
        resume_at: Some((x, y)),
    }
}

/// Approved design (FORK.md F8): the cursor is drawn after the guessed
/// letters, so typing looks like typing, not like text appearing ahead of a
/// stuck cursor.
#[test]
fn guessed_run_moves_the_cursor_past_the_last_guess() {
    let mut state = remote_state_with_surface();
    state
        .pane_predictions
        .insert("pane_1".to_string(), confirmed_at(2, 0));

    state.record_pane_prediction("pane_1", &key_event('x'));
    state.record_pane_prediction("pane_1", &key_event('y'));

    let area = state.layout(40, 12).pane_surface;
    let composed = state.compose(40, 12).expect("composed frame");
    let cursor = composed.cursor.clone().expect("cursor");
    assert_eq!(
        (cursor.x, cursor.y),
        (area.x + 4, area.y),
        "cursor must sit right after the two guessed letters"
    );
}

/// Password safety beyond Enter: a confirmed epoch belongs to the spot where
/// the server last confirmed a guess. If the program then draws a new prompt
/// elsewhere (type-ahead echoed during a command, then `Password:`), the
/// cursor is no longer there, so nothing typed may be drawn until the server
/// proves echo again.
#[test]
fn confirmed_epoch_does_not_carry_to_a_new_prompt_position() {
    let mut state = remote_state_with_surface();
    state
        .pane_predictions
        .insert("pane_1".to_string(), confirmed_at(2, 0));

    state.set_pane_surface(remote_surface_with_cursor_at(5, 1, 2));
    for ch in ['s', 'e', 'c'] {
        state.record_pane_prediction("pane_1", &key_event(ch));
        let area = state.layout(40, 12).pane_surface;
        let composed = state.compose(40, 12).expect("composed frame");
        let buffer = composed.to_ratatui_buffer().expect("ratatui buffer");
        for y in 0..2u16 {
            for x in 0..10u16 {
                assert_eq!(
                    buffer[(area.x + x, area.y + y)].symbol().trim(),
                    "",
                    "a typed character was drawn at a new prompt the server never echoed at"
                );
            }
        }
    }
}

/// The client loop (`src/client/mod.rs`) only composes a new frame after
/// input when the input outcome asks for a repaint. A guess that does not ask
/// is recorded and never drawn until the server's echo arrives anyway, which
/// makes the whole feature invisible in real use while every test that calls
/// `compose()` itself still passes. Observed 2026-09-29: remote typing median
/// 54 ms with and without the feature.
#[test]
fn typing_a_drawn_guess_requests_a_repaint() {
    let mut state = remote_state_with_surface();
    state
        .pane_predictions
        .insert("pane_1".to_string(), confirmed_at(2, 0));

    let key = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Char('x'),
        crossterm::event::KeyModifiers::empty(),
    );
    let outcome = state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Key(key)]);

    assert!(
        outcome.repaint,
        "a drawn guess must ask the client loop to compose, or it is never shown"
    );
}

/// The password rule's other half: a guess that is tracked but not drawn
/// (unconfirmed epoch) must not force a repaint on every keystroke.
#[test]
fn typing_an_undrawn_guess_does_not_request_a_repaint() {
    let mut state = remote_state_with_surface();
    let key = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Char('x'),
        crossterm::event::KeyModifiers::empty(),
    );
    let outcome = state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Key(key)]);
    assert!(!outcome.repaint);
}

/// A server patch that echoes `ch` at column `x` of row 0, the shape the
/// server sends while you type (a changed row span plus the new cursor).
fn echo_patch(state: &ClientShellState, ch: char, x: u16) -> crate::protocol::PaneSurfacePatch {
    let current = state.pane_surface.as_ref().expect("surface");
    let mut cell = current.frame.cells[usize::from(x)].clone();
    cell.symbol = ch.to_string();
    crate::protocol::PaneSurfacePatch {
        boot_id: current.boot_id.clone(),
        projection_revision: current.projection_revision,
        base_surface_revision: current.surface_revision,
        surface_revision: current.surface_revision + 1,
        panes: current.panes.clone(),
        rows: vec![crate::protocol::PaneSurfacePatchRow {
            x,
            y: 0,
            cells: vec![cell],
        }],
        cursor: Some(crate::protocol::CursorState {
            x: x + 1,
            y: 0,
            visible: true,
            shape: 2,
        }),
    }
}

/// Server echoes usually arrive as surface patches presented straight to the
/// terminal without `compose()`. Confirmation must happen where the patch
/// lands, or the epoch never becomes confirmed in real use and no guess is
/// ever drawn. Observed 2026-09-29 in a federated client against `ssh-joel`:
/// guesses recorded, cursor advanced by the echo, zero reconciliations.
#[test]
fn server_patch_confirms_the_epoch_without_a_compose() {
    let mut state = remote_state_with_surface();
    let _ = state.compose(40, 12);
    state.record_pane_prediction("pane_1", &key_event('z'));

    let patch = echo_patch(&state, 'z', 2);
    let _ = state.apply_pane_surface_patch(patch);

    let prediction = state
        .pane_predictions
        .get("pane_1")
        .expect("prediction kept");
    assert_eq!(
        prediction.epoch,
        super::super::predict::PredictionEpoch::Confirmed
    );
    assert_eq!(prediction.resume_at, Some((3, 0)));
}

/// While a guess is drawn, a fast-path patch would repaint the row from the
/// server's content and wipe the faint letters still ahead of the echo, or
/// leave a mismatched one on screen. So the patch must ask for a full compose.
#[test]
fn patch_while_a_guess_is_drawn_falls_back_to_a_full_compose() {
    let mut state = remote_state_with_surface();
    let _ = state.compose(40, 12);
    state
        .pane_predictions
        .insert("pane_1".to_string(), confirmed_at(2, 0));
    state.record_pane_prediction("pane_1", &key_event('x'));
    state.record_pane_prediction("pane_1", &key_event('y'));

    let patch = echo_patch(&state, 'x', 2);
    let outcome = state.apply_pane_surface_patch(patch);

    assert!(
        matches!(
            outcome,
            super::super::surface_patch::ClientPaneSurfacePatchOutcome::Applied(None)
        ),
        "a patch landing while guesses are drawn must fall back to compose"
    );
}
