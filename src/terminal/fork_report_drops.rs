// Fork-owned. Makes every silently dropped agent state report observable.
//
// `TerminalState::set_hook_authority_at` and `route_full_lifecycle_hook_report`
// (both in `state.rs`) return `None`/`Ignore` from many branches, and until
// this file existed nothing said why: `pane.report_agent` still answered
// `ok`, and `agent explain` hardcoded `skipped_update_reason: null`. See
// `docs/findings/2026-08-28-suppression-latch-drops-agent-reports.md` (0039).
//
// Pure observability: this file adds no new Ignore/None branch and changes no
// routing decision, it only names the branch that already fired.

use std::time::Instant;

use crate::detect::AgentState;

use super::TerminalState;

/// Why an agent state report never became (or stayed) the hook authority.
/// One variant per drop site in `state.rs`. `as_str()` is the stable,
/// snake_case name surfaced over the wire in `agent explain`'s
/// `skipped_update_reason` and in the `reason` field of the
/// `"dropped agent state report"` log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentReportDropReason {
    /// `set_hook_authority_at`: source only ever supplies session identity,
    /// never full lifecycle state.
    SessionIdentityOnlyIntegration,
    /// `set_hook_authority_at`: a non-lifecycle source reported the same
    /// agent that a process exit was just observed for.
    RecentProcessExitNonLifecycle,
    /// `route_full_lifecycle_hook_report`: report matches a session already
    /// recorded stale for this source.
    StaleSession,
    /// `route_full_lifecycle_hook_report`: `herdr:opencode` cross-talk, a
    /// different session than the one anchored while the process is present.
    OpencodeCrossTalk,
    /// `route_full_lifecycle_hook_report`: source is suppressed under a
    /// different agent label than the one reporting now.
    SuppressedDifferentAgentLabel,
    /// `route_full_lifecycle_hook_report`: source is HookClear-suppressed and
    /// the incoming report did not reanchor to a new session.
    HookClearSuppressionWithoutReanchor,
    /// `route_full_lifecycle_hook_report`: not process-present/session-
    /// anchored and the report carries no session ref.
    MissingSessionRef,
    /// `route_full_lifecycle_hook_report`: not process-present/session-
    /// anchored and the report carries no seq.
    MissingSeq,
    /// `route_full_lifecycle_hook_report`: seq is not newer than the last one
    /// accepted for this source.
    SeqNotNewer,
    /// `route_full_lifecycle_hook_report`: report was stashed as a pending
    /// replacement while the source is ProcessExit-suppressed (the 0039
    /// shape: latch never clears because `detected_agent` never changes).
    SuppressedProcessExitPending,
    /// `set_hook_authority_at`: agent label conflicts with the currently
    /// detected agent.
    KnownAgentLabelConflict,
    /// `set_hook_authority_at`: a different owner already holds this session
    /// and no foreground takeover was confirmed.
    OwnerConflict,
    /// `set_hook_authority_at`: a live full-lifecycle authority conflicts
    /// with the incoming session.
    LiveAuthoritySessionConflict,
    /// `set_hook_authority_at`: `accept_hook_report` rejected the seq.
    AcceptHookReportSeqRejected,
    /// `release_agent_with_mutation`: current hook authority belongs to a
    /// different source/agent label than the one releasing.
    ReleaseAuthorityMismatch,
    /// `release_agent_with_mutation`: neither the current effective agent nor
    /// the persisted session matches the one releasing.
    ReleaseNotCurrentOrPersistedSession,
    /// `release_agent_with_mutation`: `accept_hook_report` rejected the seq.
    ReleaseSeqRejected,
}

impl AgentReportDropReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionIdentityOnlyIntegration => "session_identity_only_integration",
            Self::RecentProcessExitNonLifecycle => "recent_process_exit_non_lifecycle",
            Self::StaleSession => "stale_session",
            Self::OpencodeCrossTalk => "opencode_cross_talk",
            Self::SuppressedDifferentAgentLabel => "suppressed_different_agent_label",
            Self::HookClearSuppressionWithoutReanchor => "hook_clear_suppression_without_reanchor",
            Self::MissingSessionRef => "missing_session_ref",
            Self::MissingSeq => "missing_seq",
            Self::SeqNotNewer => "seq_not_newer",
            Self::SuppressedProcessExitPending => "suppressed_process_exit_pending",
            Self::KnownAgentLabelConflict => "known_agent_label_conflict",
            Self::OwnerConflict => "owner_conflict",
            Self::LiveAuthoritySessionConflict => "live_authority_session_conflict",
            Self::AcceptHookReportSeqRejected => "accept_hook_report_seq_rejected",
            Self::ReleaseAuthorityMismatch => "release_authority_mismatch",
            Self::ReleaseNotCurrentOrPersistedSession => "release_not_current_or_persisted_session",
            Self::ReleaseSeqRejected => "release_seq_rejected",
        }
    }
}

/// The last dropped report for a terminal, cleared as soon as a report is
/// accepted. One slot, not a log: `agent explain` reads only "why is the
/// pane stuck right now", not history.
#[derive(Debug, Clone)]
pub struct DroppedAgentReport {
    pub reason: AgentReportDropReason,
    // Only `reason` crosses the wire today (`last_dropped_report_reason`).
    // These three are read by the `#[cfg(test)]`-only `last_dropped_report`
    // accessor below, so a non-test build never reads them back - stored for
    // the fork contract test and future diagnostics, not dead by mistake.
    #[allow(dead_code)]
    pub source: String,
    #[allow(dead_code)]
    pub agent_label: String,
    #[allow(dead_code)]
    pub when: Instant,
}

impl TerminalState {
    /// Record why a report was dropped and emit the one `tracing::info!` line
    /// for it. Call at the point of the `None`/`Ignore` return, never after
    /// forwarding an already-recorded `Ignore` from another function (that
    /// would log the same drop twice).
    pub(crate) fn record_dropped_report(
        &mut self,
        reason: AgentReportDropReason,
        source: &str,
        agent_label: &str,
        state: Option<AgentState>,
    ) {
        tracing::info!(
            terminal = %self.id,
            source,
            agent = agent_label,
            state = state.map(crate::detect::manifest::agent_state_label),
            reason = reason.as_str(),
            "dropped agent state report"
        );
        self.last_dropped_report = Some(DroppedAgentReport {
            reason,
            source: source.to_string(),
            agent_label: agent_label.to_string(),
            when: Instant::now(),
        });
    }

    /// Clear the recorded drop. Call when a report is accepted, so `agent
    /// explain` never blames a stale drop for a pane that has since updated.
    pub(crate) fn clear_dropped_report(&mut self) {
        self.last_dropped_report = None;
    }

    /// The stable snake_case name of the last dropped report, if any, for
    /// `agent explain`'s `skipped_update_reason`.
    pub fn last_dropped_report_reason(&self) -> Option<&'static str> {
        self.last_dropped_report
            .as_ref()
            .map(|dropped| dropped.reason.as_str())
    }

    /// The full record of the last dropped report, if any. Crate-internal:
    /// only the reason crosses the wire (`last_dropped_report_reason`), the
    /// rest is for tests and future diagnostics.
    #[cfg(test)]
    pub(crate) fn last_dropped_report(&self) -> Option<&DroppedAgentReport> {
        self.last_dropped_report.as_ref()
    }
}
