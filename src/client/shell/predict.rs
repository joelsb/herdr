//! Predictive local echo (mosh-style) for panes on a remote endpoint.
//!
//! See `FORK.md` F8 for the approved design, the password-safety epoch, and
//! the replication hazard for the three upstream files this hooks into
//! (`input.rs`, `composition.rs`, `state.rs::timer_delay`). Fork-owned file:
//! never move an assertion about this feature into an upstream-owned test
//! file (`src/client/shell/tests/*` files this repo did not originate are
//! still fine to add a *new* sibling test file to - only don't fold these
//! assertions into an existing upstream test module).
use super::*;
use unicode_width::UnicodeWidthChar;

/// How long an unconfirmed guess may sit on screen before predictive echo
/// gives up on it and clears the whole pane's guesses.
///
/// ponytail: no server input-ack exists yet to confirm this number is right
/// under a slow link; when the server gains a per-key input ack, this
/// constant and the whole confirm-by-content-match path in this file can be
/// replaced by confirm-by-ack, which would not need a timeout at all.
pub(super) const PREDICTIVE_ECHO_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(250);

/// Per-pane confirmation state. Unconfirmed guesses are tracked but never
/// drawn, so a password prompt with no echo never shows a typed character:
/// the epoch only ever advances to `Confirmed` when a real server frame
/// shows a guessed character at its predicted cell, and drops back to
/// `Unconfirmed` on Enter, any clearing key, or a mismatch (FORK.md F8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum PredictionEpoch {
    #[default]
    Unconfirmed,
    Confirmed,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PredictedGuess {
    pub(super) x: u16,
    pub(super) y: u16,
    pub(super) ch: char,
    pub(super) requested_at: std::time::Instant,
}

#[derive(Debug, Clone, Default)]
pub(super) struct PanePrediction {
    pub(super) epoch: PredictionEpoch,
    pub(super) guesses: Vec<PredictedGuess>,
}

impl ClientShellState {
    /// Predictive echo only ever applies to a pane on a remote endpoint -
    /// `self.active_endpoint_id` is always the endpoint the currently
    /// composed snapshot/pane surface (and therefore any typed-into pane)
    /// belongs to, so this single check is enough gating without needing to
    /// search `self.endpoints` for the pane's owner.
    fn predictive_echo_eligible(&self, pane_id: &str) -> bool {
        self.config.predictive_echo
            && !self.active_endpoint_id.is_local()
            && self
                .snapshot
                .as_deref()
                .is_some_and(|snapshot| snapshot.panes.iter().any(|pane| pane.pane_id == pane_id))
    }

    /// Single choke point for every input event headed to a pane (called
    /// from both `push_pane_key` and `push_focused_pane_event` in
    /// `input.rs`, before the event is forwarded to the pane). Mouse events
    /// and anything else not explicitly guessable clear the pane's guesses,
    /// fail-closed by default.
    pub(super) fn record_pane_prediction(
        &mut self,
        pane_id: &str,
        event: &crate::protocol::ClientPaneInputEvent,
    ) {
        match event {
            crate::protocol::ClientPaneInputEvent::Key {
                code,
                modifiers,
                kind,
                ..
            } => self.record_predicted_key(pane_id, code, *modifiers, *kind),
            crate::protocol::ClientPaneInputEvent::TextCommit(text) => {
                self.record_predicted_text(pane_id, text);
            }
            crate::protocol::ClientPaneInputEvent::Mouse { .. }
            | crate::protocol::ClientPaneInputEvent::Paste(_) => {
                self.clear_predictions(pane_id);
            }
        }
    }

    fn record_predicted_key(
        &mut self,
        pane_id: &str,
        code: &crate::protocol::ClientKeyCode,
        modifiers: u8,
        kind: crate::protocol::ClientKeyKind,
    ) {
        if kind != crate::protocol::ClientKeyKind::Press
            || !self.predictive_echo_eligible(pane_id)
        {
            return;
        }
        let chord = crossterm::event::KeyModifiers::from_bits_truncate(modifiers).intersects(
            crossterm::event::KeyModifiers::CONTROL
                | crossterm::event::KeyModifiers::ALT
                | crossterm::event::KeyModifiers::SUPER
                | crossterm::event::KeyModifiers::HYPER
                | crossterm::event::KeyModifiers::META,
        );
        match code {
            crate::protocol::ClientKeyCode::Char(ch) if !chord => self.guess_char(pane_id, *ch),
            crate::protocol::ClientKeyCode::Backspace if !chord => {
                self.guess_backspace(pane_id);
            }
            _ => self.clear_predictions(pane_id),
        }
    }

    fn record_predicted_text(&mut self, pane_id: &str, text: &str) {
        if !self.predictive_echo_eligible(pane_id) {
            return;
        }
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => self.guess_char(pane_id, ch),
            _ => self.clear_predictions(pane_id),
        }
    }

    pub(super) fn clear_predictions(&mut self, pane_id: &str) {
        self.pane_predictions.remove(pane_id);
    }

    fn guess_char(&mut self, pane_id: &str, ch: char) {
        if UnicodeWidthChar::width(ch) != Some(1) {
            // Wide or combining: never guessed, and it invalidates whatever
            // run was in flight for this pane.
            self.clear_predictions(pane_id);
            return;
        }
        let Some((x, y)) = self.next_guess_position(pane_id) else {
            // Not at end-of-row, out of pane width, or no surface yet:
            // simply do not guess. This is not a mismatch, so existing
            // guesses (if any) are left alone.
            return;
        };
        self.pane_predictions
            .entry(pane_id.to_string())
            .or_default()
            .guesses
            .push(PredictedGuess {
                x,
                y,
                ch,
                requested_at: std::time::Instant::now(),
            });
    }

    fn guess_backspace(&mut self, pane_id: &str) {
        if let Some(prediction) = self.pane_predictions.get_mut(pane_id) {
            prediction.guesses.pop();
        }
    }

    fn pane_inner_rect(&self, pane_id: &str) -> Option<crate::protocol::SurfaceRect> {
        self.pane_surface
            .as_ref()?
            .panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .map(|pane| pane.inner_rect)
    }

    /// Where the next guessed character would land: continuing a pending
    /// run advances one cell past the last guess; starting a fresh run
    /// requires the real cursor to sit at the end of text on its row (every
    /// cell to the right of it, inclusive, blank) with room left in the
    /// pane. Returns `None` when neither holds - never guessed, per FORK.md
    /// F8.
    fn next_guess_position(&self, pane_id: &str) -> Option<(u16, u16)> {
        let inner = self.pane_inner_rect(pane_id)?;
        let right = inner.x.saturating_add(inner.width);
        if let Some(last) = self
            .pane_predictions
            .get(pane_id)
            .and_then(|prediction| prediction.guesses.last())
        {
            let x = last.x.checked_add(1)?;
            return (x < right).then_some((x, last.y));
        }
        let surface = self.pane_surface.as_ref()?;
        let cursor = surface.frame.cursor.as_ref()?;
        let bottom = inner.y.saturating_add(inner.height);
        if cursor.x < inner.x || cursor.x >= right || cursor.y < inner.y || cursor.y >= bottom {
            return None;
        }
        let row_start = usize::from(cursor.y) * usize::from(surface.frame.width);
        let tail_blank = (cursor.x..right).all(|x| {
            surface
                .frame
                .cells
                .get(row_start + usize::from(x))
                .is_none_or(|cell| cell.symbol.trim().is_empty())
        });
        tail_blank.then_some((cursor.x, cursor.y))
    }

    /// Confirm or invalidate pending guesses against the real pane surface,
    /// resolving each pane's run strictly from the front: a later guess
    /// cannot be known-good while an earlier one on the same row is still
    /// unresolved (blank). Mutating - called from `compose()`'s existing
    /// mutating prep phase, never from the pure cell-drawing step below
    /// (FORK.md F8: "render stays pure").
    pub(super) fn reconcile_predictions(&mut self) {
        if self.pane_surface.is_none() {
            self.pane_predictions.clear();
            return;
        }
        let frame = self.pane_surface.as_ref().unwrap().frame.clone();
        let pane_ids: Vec<String> = self.pane_predictions.keys().cloned().collect();
        for pane_id in pane_ids {
            self.reconcile_pane(&pane_id, &frame);
        }
    }

    fn reconcile_pane(&mut self, pane_id: &str, frame: &FrameData) {
        let Some(prediction) = self.pane_predictions.get_mut(pane_id) else {
            return;
        };
        let mut mismatch = false;
        let mut confirmed_any = false;
        while let Some(first) = prediction.guesses.first().copied() {
            let index = usize::from(first.y) * usize::from(frame.width) + usize::from(first.x);
            let Some(cell) = frame.cells.get(index) else {
                mismatch = true;
                break;
            };
            if cell.symbol.trim().is_empty() {
                // Still unresolved: the server has not echoed this cell
                // yet. Stop scanning - later guesses in the run cannot
                // resolve ahead of this one.
                break;
            }
            if cell.symbol == first.ch.to_string() {
                confirmed_any = true;
                prediction.guesses.remove(0);
            } else {
                mismatch = true;
                break;
            }
        }
        if mismatch {
            self.pane_predictions.remove(pane_id);
            return;
        }
        if confirmed_any {
            prediction.epoch = PredictionEpoch::Confirmed;
        }
    }

    /// Drop any pane's guesses that have sat unconfirmed past
    /// [`PREDICTIVE_ECHO_TIMEOUT`]. Returns whether anything changed, so a
    /// caller can decide a repaint is warranted even with no other input.
    pub(super) fn tick_predictive_echo(&mut self, now: std::time::Instant) -> bool {
        let before = self.pane_predictions.len();
        self.pane_predictions.retain(|_, prediction| {
            !prediction
                .guesses
                .iter()
                .any(|guess| now.duration_since(guess.requested_at) >= PREDICTIVE_ECHO_TIMEOUT)
        });
        before != self.pane_predictions.len()
    }

    /// Earliest instant at which an outstanding guess will time out, folded
    /// into `timer_delay()` so the client wakes to prune it promptly even
    /// when nothing else is happening.
    pub(super) fn next_predictive_echo_expiry(&self) -> Option<std::time::Instant> {
        self.pane_predictions
            .values()
            .flat_map(|prediction| prediction.guesses.iter())
            .map(|guess| guess.requested_at + PREDICTIVE_ECHO_TIMEOUT)
            .min()
    }

    /// Draw confirmed-epoch guesses as dim cells onto the already-composed
    /// frame, at the offset the pane surface itself was blitted to. Purely
    /// a read over `self.pane_predictions`: every state change happens
    /// earlier, in `reconcile_predictions`/`tick_predictive_echo`.
    pub(super) fn overlay_predictive_echo(&self, frame: &mut FrameData, area: Rect) {
        if self.pane_predictions.is_empty() {
            return;
        }
        for prediction in self.pane_predictions.values() {
            if prediction.epoch != PredictionEpoch::Confirmed {
                continue;
            }
            for guess in &prediction.guesses {
                let x = area.x.saturating_add(guess.x);
                let y = area.y.saturating_add(guess.y);
                if x >= frame.width || y >= frame.height {
                    continue;
                }
                let index = usize::from(y) * usize::from(frame.width) + usize::from(x);
                if let Some(cell) = frame.cells.get_mut(index) {
                    cell.symbol = guess.ch.to_string();
                    cell.modifier |= Modifier::DIM.bits();
                }
            }
        }
    }
}
