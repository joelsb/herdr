use crate::terminal::TerminalId;
use std::time::Instant;

/// Viewport state for a pane.
///
/// Terminal identity, cwd, labels, and agent metadata live in TerminalState.
pub struct PaneState {
    pub attached_terminal_id: TerminalId,
    /// Whether the user has seen this pane since its last state change to Idle.
    /// False = "Done" (agent finished while user was in another workspace).
    pub seen: bool,
    /// When the user last looked at this pane.
    ///
    /// Distinct from the terminal's `state_entered_at`: an unseen pane is aged
    /// from when its result appeared, but a seen pane is aged from the last
    /// look, so glancing at a pane keeps it out of the parked bucket. Advances
    /// on every look, including re-focusing an already-seen pane.
    pub seen_at: Instant,
    /// Whether the "finished result has gone unread" alert already fired for
    /// the current idle episode. Cleared on every look and whenever the
    /// terminal leaves Idle, so one episode raises at most one alert.
    pub stale_notified: bool,
    /// Whether unmodified right-click gestures should be forwarded to the pane application.
    pub right_click_passthrough: bool,
}

impl PaneState {
    pub fn new(attached_terminal_id: TerminalId) -> Self {
        Self {
            attached_terminal_id,
            seen: true,
            seen_at: Instant::now(),
            stale_notified: false,
            right_click_passthrough: false,
        }
    }

    /// Record that the user looked at this pane.
    ///
    /// Returns true when this flipped an unseen pane to seen, matching the
    /// change-detection the callers already rely on. The timestamp advances
    /// either way, and re-arms the unread alert, because looking again is a
    /// fresh look even if the pane was already marked seen.
    pub fn mark_seen(&mut self, now: Instant) -> bool {
        let was_unseen = !self.seen;
        self.seen = true;
        self.seen_at = now;
        self.stale_notified = false;
        was_unseen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn mark_seen_advances_the_look_clock_every_time() {
        let mut pane = PaneState::new(TerminalId::alloc());
        let t0 = Instant::now();

        pane.seen = false;
        pane.seen_at = t0;
        assert!(pane.mark_seen(t0 + Duration::from_secs(10)));
        assert!(pane.seen);
        assert_eq!(pane.seen_at, t0 + Duration::from_secs(10));

        // Re-focusing an already-seen pane is still a look, so the clock has to
        // move: otherwise a pane you keep checking would still go parked.
        assert!(!pane.mark_seen(t0 + Duration::from_secs(20)));
        assert_eq!(pane.seen_at, t0 + Duration::from_secs(20));
    }

    #[test]
    fn mark_seen_rearms_the_unread_alert() {
        let mut pane = PaneState::new(TerminalId::alloc());
        pane.stale_notified = true;
        pane.mark_seen(Instant::now());
        assert!(!pane.stale_notified);
    }
}
