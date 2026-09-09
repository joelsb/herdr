use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

use super::widgets::panel_contrast_fg;
use crate::{
    app::state::{CopyFeedback, Palette},
    config::ToastClipboardPosition,
};

pub(crate) fn copy_feedback_rect(
    area: Rect,
    feedback: &CopyFeedback,
    offset_rows: u16,
    position: ToastClipboardPosition,
) -> Rect {
    if area.is_empty() {
        return Rect::default();
    }

    let content_width = feedback.message.len() as u16 + 4;
    let width = content_width.min(area.width);
    let height = 3u16.min(area.height);
    let x = match position {
        ToastClipboardPosition::TopLeft | ToastClipboardPosition::BottomLeft => area.x,
        ToastClipboardPosition::TopCenter | ToastClipboardPosition::BottomCenter => {
            area.x + area.width.saturating_sub(width) / 2
        }
        ToastClipboardPosition::TopRight | ToastClipboardPosition::BottomRight => {
            area.x + area.width.saturating_sub(width)
        }
    };
    let y = match position {
        ToastClipboardPosition::TopLeft
        | ToastClipboardPosition::TopCenter
        | ToastClipboardPosition::TopRight => area.y + offset_rows.min(area.height),
        ToastClipboardPosition::BottomLeft
        | ToastClipboardPosition::BottomCenter
        | ToastClipboardPosition::BottomRight => {
            area.y + area.height.saturating_sub(height + offset_rows)
        }
    };
    Rect::new(x, y, width, height)
}

pub(crate) fn render_copy_feedback_buffer(
    buffer: &mut Buffer,
    area: Rect,
    feedback: &CopyFeedback,
    offset_rows: u16,
    position: ToastClipboardPosition,
    palette: &Palette,
) {
    let feedback_area = copy_feedback_rect(area, feedback, offset_rows, position);
    if feedback_area.is_empty() {
        return;
    }

    Clear.render(feedback_area, buffer);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.green))
        .style(Style::default().bg(palette.panel_bg));
    let inner = block.inner(feedback_area);
    block.render(feedback_area, buffer);

    if inner.height == 0 {
        return;
    }

    let text = Line::from(vec![
        Span::styled("●", Style::default().fg(palette.green).bg(palette.panel_bg)),
        Span::raw(" "),
        Span::styled(
            &feedback.message,
            Style::default()
                .fg(palette.text)
                .bg(palette.panel_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    Paragraph::new(text).render(inner, buffer);
}

pub(crate) fn render_config_diagnostic_buffer(
    buffer: &mut Buffer,
    area: Rect,
    message: &str,
    palette: &Palette,
) -> u16 {
    let style = Style::default()
        .fg(panel_contrast_fg(palette))
        .bg(palette.yellow)
        .add_modifier(Modifier::BOLD);
    let mut rendered_rows = 0u16;

    for (row, line) in message
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(area.height as usize)
        .enumerate()
    {
        let text = format!(" {line} ");
        let width = (text.len() as u16).min(area.width);
        let diagnostic_area = Rect::new(
            area.x + area.width.saturating_sub(width),
            area.y + row as u16,
            width,
            1,
        );

        Clear.render(diagnostic_area, buffer);
        Paragraph::new(Span::styled(text, style)).render(diagnostic_area, buffer);
        rendered_rows = rendered_rows.saturating_add(1);
    }

    rendered_rows
}

/// How long an idle pane has been that way, and whether the user has looked.
///
/// This replaces the older bare `seen: bool` at the presentation boundary: it
/// carries the same acknowledgement fact plus the age, and being one value
/// makes the contradictory `(seen = true, aged-from-result-time)` pair
/// unrepresentable.
///
/// Two clocks feed it, because the two halves ask different questions. An
/// unseen pane is aged from when its result appeared, so the age means "how
/// long has this been sitting unread". A seen pane is aged from the last look,
/// so glancing at a pane keeps it out of the parked bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleAge {
    /// Finished recently, user has not looked yet.
    FreshUnseen,
    /// Finished a while ago and the user still has not looked.
    StaleUnseen,
    /// User looked recently; still in their working set.
    FreshSeen,
    /// User looked a while ago and left it alone; deliberately parked.
    ParkedSeen,
}

impl IdleAge {
    /// Whether this age is worth interrupting the user about.
    ///
    /// Only an unread result qualifies. A pane the user looked at and left is a
    /// deliberate choice, so parking it silently is the point.
    pub(crate) fn warrants_unread_alert(self) -> bool {
        matches!(self, Self::StaleUnseen)
    }
}

/// Resolve an idle age for one pane at render time.
///
/// `aged_from` is whichever clock applies, chosen by the server side; this only
/// measures it. Takes a caller-supplied `now` so a whole frame classifies
/// against one instant instead of re-reading the clock per pane.
pub(crate) fn idle_age_at(
    app: &crate::app::AppState,
    seen: bool,
    aged_from: std::time::Instant,
    now: std::time::Instant,
) -> IdleAge {
    idle_age_for(
        seen,
        now.saturating_duration_since(aged_from),
        app.idle_stale_after,
    )
}

/// Classify an idle pane from its acknowledgement and the elapsed time on
/// whichever clock applies.
///
/// The caller picks the clock, because only it knows which timestamp belongs to
/// which half: `state_entered_at` for an unseen pane, `seen_at` for a seen one.
/// The boundary lands on the aged side, so at exactly the threshold a result
/// counts as having sat long enough.
pub(crate) fn idle_age_for(
    seen: bool,
    elapsed: std::time::Duration,
    threshold: std::time::Duration,
) -> IdleAge {
    match (seen, elapsed >= threshold) {
        (false, false) => IdleAge::FreshUnseen,
        (false, true) => IdleAge::StaleUnseen,
        (true, false) => IdleAge::FreshSeen,
        (true, true) => IdleAge::ParkedSeen,
    }
}

pub(super) fn state_icon_symbol(
    state: AgentState,
    age: IdleAge,
    indicator_style: StatusIndicatorStyle,
) -> &'static str {
    match (indicator_style, state, age) {
        (StatusIndicatorStyle::Dots, AgentState::Blocked, _) => "●",
        (StatusIndicatorStyle::Dots, AgentState::Working, _) => "●",
        (StatusIndicatorStyle::Dots, AgentState::Idle, IdleAge::FreshUnseen) => "●",
        (StatusIndicatorStyle::Dots, AgentState::Idle, IdleAge::StaleUnseen) => "◉",
        (StatusIndicatorStyle::Dots, AgentState::Idle, IdleAge::FreshSeen) => "○",
        (StatusIndicatorStyle::Dots, AgentState::Idle, IdleAge::ParkedSeen) => "◌",
        (StatusIndicatorStyle::Dots, AgentState::Unknown, _) => "·",
        (StatusIndicatorStyle::Symbols, AgentState::Blocked, _) => "×",
        (StatusIndicatorStyle::Symbols, AgentState::Working, _) => "◐",
        (StatusIndicatorStyle::Symbols, AgentState::Idle, IdleAge::FreshUnseen) => "✓",
        (StatusIndicatorStyle::Symbols, AgentState::Idle, IdleAge::StaleUnseen) => "!",
        (StatusIndicatorStyle::Symbols, AgentState::Idle, IdleAge::FreshSeen) => "○",
        (StatusIndicatorStyle::Symbols, AgentState::Idle, IdleAge::ParkedSeen) => "◌",
        (StatusIndicatorStyle::Symbols, AgentState::Unknown, _) => "·",
    }
}

pub(super) fn state_icon(
    state: AgentState,
    age: IdleAge,
    indicator_style: StatusIndicatorStyle,
    p: &Palette,
) -> (&'static str, Style) {
    (
        state_icon_symbol(state, age, indicator_style),
        Style::default().fg(state_label_color(state, age, p)),
    )
}

pub(super) fn state_label(state: AgentState, age: IdleAge) -> &'static str {
    match (state, age) {
        (AgentState::Blocked, _) => "blocked",
        (AgentState::Working, _) => "working",
        (AgentState::Idle, IdleAge::FreshUnseen) => "done",
        (AgentState::Idle, IdleAge::StaleUnseen) => "stale",
        (AgentState::Idle, IdleAge::FreshSeen) => "idle",
        (AgentState::Idle, IdleAge::ParkedSeen) => "parked",
        (AgentState::Unknown, _) => "idle",
    }
}

pub(super) fn state_label_color(state: AgentState, age: IdleAge, p: &Palette) -> Color {
    match (state, age) {
        (AgentState::Blocked, _) => p.red,
        (AgentState::Working, _) => p.yellow,
        (AgentState::Idle, IdleAge::FreshUnseen) => p.teal,
        (AgentState::Idle, IdleAge::StaleUnseen) => p.peach,
        (AgentState::Idle, IdleAge::FreshSeen) => p.green,
        (AgentState::Idle, IdleAge::ParkedSeen) => p.overlay0,
        (AgentState::Unknown, _) => p.overlay0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn feedback() -> CopyFeedback {
        CopyFeedback {
            message: "copied to clipboard".to_string(),
        }
    }

    #[test]
    fn state_icons_support_dot_and_distinct_symbol_styles() {
        let palette = Palette::catppuccin();
        for (indicator_style, expected_symbols) in [
            (
                StatusIndicatorStyle::Dots,
                ["●", "●", "●", "◉", "○", "◌", "·"],
            ),
            (
                StatusIndicatorStyle::Symbols,
                ["×", "◐", "✓", "!", "○", "◌", "·"],
            ),
        ] {
            for ((state, age, color, label), expected_symbol) in [
                (
                    AgentState::Blocked,
                    IdleAge::FreshSeen,
                    palette.red,
                    "blocked",
                ),
                (
                    AgentState::Working,
                    IdleAge::FreshSeen,
                    palette.yellow,
                    "working",
                ),
                (AgentState::Idle, IdleAge::FreshUnseen, palette.teal, "done"),
                (
                    AgentState::Idle,
                    IdleAge::StaleUnseen,
                    palette.peach,
                    "stale",
                ),
                (AgentState::Idle, IdleAge::FreshSeen, palette.green, "idle"),
                (
                    AgentState::Idle,
                    IdleAge::ParkedSeen,
                    palette.overlay0,
                    "parked",
                ),
                (
                    AgentState::Unknown,
                    IdleAge::FreshSeen,
                    palette.overlay0,
                    "idle",
                ),
            ]
            .into_iter()
            .zip(expected_symbols)
            {
                let (actual_symbol, style) = state_icon(state, age, indicator_style, &palette);
                assert_eq!(actual_symbol, expected_symbol);
                assert_eq!(display_width_u16(actual_symbol), 1);
                assert_eq!(style.fg, Some(color));
                assert_eq!(state_label(state, age), label);
            }
        }
    }

    #[test]
    fn idle_age_classifies_by_the_right_clock() {
        let threshold = Duration::from_secs(300);

        assert_eq!(
            idle_age_for(false, Duration::from_secs(299), threshold),
            IdleAge::FreshUnseen
        );
        assert_eq!(
            idle_age_for(true, Duration::from_secs(299), threshold),
            IdleAge::FreshSeen
        );

        // The boundary lands on the aged side: at exactly the threshold the
        // result has been sitting long enough to count.
        assert_eq!(
            idle_age_for(false, threshold, threshold),
            IdleAge::StaleUnseen
        );
        assert_eq!(
            idle_age_for(true, threshold, threshold),
            IdleAge::ParkedSeen
        );

        assert_eq!(
            idle_age_for(false, Duration::from_secs(3600), threshold),
            IdleAge::StaleUnseen
        );
        assert_eq!(
            idle_age_for(true, Duration::from_secs(3600), threshold),
            IdleAge::ParkedSeen
        );
    }

    #[test]
    fn only_an_unseen_stale_pane_is_worth_alerting_about() {
        // The alert exists for results the user has never looked at. A seen
        // pane going parked is a deliberate choice, not something to interrupt.
        assert!(IdleAge::StaleUnseen.warrants_unread_alert());
        assert!(!IdleAge::FreshUnseen.warrants_unread_alert());
        assert!(!IdleAge::FreshSeen.warrants_unread_alert());
        assert!(!IdleAge::ParkedSeen.warrants_unread_alert());
    }

    #[test]
    fn copy_feedback_rect_uses_configured_position() {
        let area = Rect::new(10, 20, 100, 40);
        let feedback = CopyFeedback {
            message: "copied to clipboard".to_owned(),
        };

        let top = copy_feedback_rect(area, &feedback, 0, ToastClipboardPosition::TopCenter);
        assert_eq!(top.y, area.y);
        assert_eq!(top.x, area.x + area.width.saturating_sub(top.width) / 2);

        let bottom = copy_feedback_rect(area, &feedback, 0, ToastClipboardPosition::BottomCenter);
        assert_eq!(bottom.bottom(), area.bottom());
        assert_eq!(
            bottom.x,
            area.x + area.width.saturating_sub(bottom.width) / 2
        );
    }
}
