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
