# Fork features (joelsb/herdr)

What this fork adds on top of upstream `herdrdev/herdr`, why each thing exists, how it actually works, and what a future agent must redo when merging a newer upstream.
Fork-local file. Upstream has no `FORK.md`, so it never conflicts.

## Run this first

```bash
export PATH=/var/tmp/xcrun-shim:$PATH      # see Build environment
export ZIG=/opt/homebrew/bin/zig           # Zig 0.16.0; PATH zig is 0.15.2 and too old
cargo nextest run fork_contract            # 24 tests + 1 deliberately ignored
```

Green means every fork feature in this file survived. A red test names the feature in its own doc comment; come back here, read that `## F<n>` section, and the replication hazard that most likely caused the loss.

The tests live in two **fork-owned** files that upstream does not have: `tests/fork_contract.rs` (drives the real binary, the CLI and the client socket) and `src/fork_contract_tests.rs` (crate internals, included from `src/main.rs`). **Never move a fork assertion into an upstream-owned test file.** Upstream owns `src/integration/tests.rs`, `src/detect/mod.rs`, `src/terminal/state.rs`, `src/client/shell/tests/*`, `src/persist/restore.rs`; a merge can delete a hunk inside one of those and take a fork assertion with it, unnoticed. That is not hypothetical - it is how this fork lost a feature's rendering for three weeks.

## Ground truth

| Fact | Value |
|---|---|
| origin | https://github.com/joelsb/herdr (our fork, branch `master`) |
| upstream | https://github.com/herdrdev/herdr |
| upstream point we sit on | `fff6c820` `refactor: extract libghostty-vt binding into ghostty-vt workspace crate (#4661)`, 2026-09-27 |
| our commits not in upstream | 27 (8 of them this port) |
| full suite | 3669 passed / 2 failed / 10 skipped; both failures predate the port (`cross_area_agent_process_survives_detach_and_reattach`, `api_ping pane_info_and_subscriptions_expose_done_agent_status`) |
| verified | 2026-09-27, by running the commands in this file |

Recompute any time: `git merge-base master upstream/master`, `git log --oneline $(git merge-base master upstream/master)..master`, `git diff --stat $(git merge-base master upstream/master)..master -- . ':(exclude)vendor'`.

## Commit map

| Commit | Feature |
|---|---|
| `46d8d3f3` | F1 jcode integration (hook asset, config editing, registry, detect gating) - **retired 2026-09-28, do not port** |
| `e87abeb4` | F1 session anchoring before the first state report + `scripts/verify_jcode_hook.py` - **retired 2026-09-28, do not port** |
| `b284249d` | F1 re-anchor on `jcode --resume` - **retired 2026-09-28, do not port** |
| `04278c88` | F2 `close_pane_if_idle` keybinding |
| `61587c39` | F3 pane titles survive a live handoff |
| `d6b82f42` `c54e7920` `276e8c91` `e5d4df50` | F4 idle aging (buckets, settings, API field, docs) |
| `9611e6b7` | F5 agents above spaces (**reverted 2026-09-28**, see below) + subagent nesting (server-side data, kept) |
| `860e418d` | F5 nest subagent rows under their parent with more than one machine (multi-endpoint fix) |
| `b94e4ffd` `e39543ce` `18794fbb` | v0.9.0 merge, compile/test repair, nesting re-done client-side |
| `bafc4c59` `2f41a6bc` | 2026-09-27 port: merge of upstream `fff6c820`, 11 conflicts, merge fallout |
| `a421888d` | F4 glyphs finally rendered from the client snapshot |
| `ffe76393` | F4 a seen pane ages from the look clock on the wire too |
| `b6d876ba` | fork-owned rebless of the frozen endpoint-shape fixture |
| `b9566910` `fa6d5fab` `a830597f` `eb1e9149` | the fork contract test set |
| `e065db16` | F6 a moved pane's old public id survives a live handoff |
| (this branch) | F7 accept moved off the main loop (JSB-17) |

---

## F1 - jcode integration

**Retired 2026-09-28 - do not port.** Joel no longer uses jcode. On the next upstream merge, take upstream's side for every jcode hunk and delete the jcode code rather than re-adding it. Nothing has been deleted yet in this commit - this section only records what the next port must remove.

**Files to remove/revert** (upstream owns all of these; taking upstream's version of a file upstream also touches is enough, no jcode re-add):

- `src/integration/assets/jcode/herdr-agent-state.sh` (whole asset)
- `src/integration/mod.rs` - `JCODE_HOOK_ASSET`, `JCODE_HOOK_INSTALL_NAME`, `JCODE_HOOK_EVENTS`, `JCODE_INTEGRATION_VERSION`
- `src/integration/env.rs` - `jcode_dir()`
- `src/integration/targets.rs` - `install_jcode` / `uninstall_jcode`
- `src/integration/config_edit.rs` - `build_jcode_config_with_hooks` / `remove_jcode_config_hooks`
- `src/integration/registry.rs`, `actions.rs`, `types.rs`, `src/api/schema/integrations.rs` - the `IntegrationTarget::Jcode` variant and its arity constants
- `src/cli/integration.rs` - the `Builtin(IntegrationTarget::Jcode)` arm
- `src/detect/mod.rs` - `Agent::Jcode` in `ALL` and in `full_lifecycle_hook_authority`
- `src/agent_resume.rs` + `src/persist/restore.rs` - `herdr:jcode` as an official source, and the `jcode --resume <id>` restore path
- `src/terminal/state.rs` - `("herdr:jcode", "jcode", Some("resume" | "new"))` in the session-replacement allow-list
- `scripts/verify_jcode_hook.py`

**Fork contract tests to delete** (both files): `fork_contract_install_jcode_*`, `fork_contract_uninstall_jcode_*`, `fork_contract_jcode_toml_*`, `fork_contract_jcode_is_hook_authority_*`, `fork_contract_jcode_resume_*`, `fork_contract_restore_plan_resumes_a_jcode_session`, `fork_contract_jcode_reporter_puts_correct_json_rpc_on_the_wire`.

**The frozen endpoint-shape fixture re-bless goes too.** "Fork-owned deviation: the frozen endpoint-shape fixture" below exists only because `IntegrationTarget::Jcode` changes the `integration.install` shape digest. Once `Jcode` is gone, `tests/fixtures/endpoint-method-shapes-v1.json` and the re-bless reasoning in `src/server/client_commands.rs` return to upstream's version - delete the fork's re-bless, do not carry it forward.

**What it was, for context.** jcode ran in herdr panes with `agent_status=unknown` and never appeared in the Agents panel, so this integration made it a *full lifecycle authority*: hooks owned the state, no screen detection, and the session id was persisted so a server restart relaunched `jcode --resume <id>` instead of a bare shell. `seq = time.time_ns()` and a `pane.report_agent_session` anchor before the first `pane.report_agent` were required because `route_full_lifecycle_hook_report` silently discards an unanchored report - see F6's note below and finding 0039, which this integration's exact shape first surfaced.

**Verify (while it still exists).** `cargo nextest run fork_contract` covers install, idempotent reinstall, uninstall preserving a foreign hook command, TOML round-trip in both string forms, the hook-authority-without-manifest gate, the resume re-anchor, the resume restore plan, and the actual JSON-RPC the script puts on a stand-in socket.

---

## F2 - `close_pane_if_idle`

**Purpose.** One bare chord that first reaches the agent and then closes the pane. `alt+x` exits jcode; press it again and the now-idle pane closes.

**Intent.** A plain `close_pane` on a bare chord would kill a pane while the agent still runs. This binding only acts when the pane is free, otherwise the key is forwarded untouched.

**How it works.** `src/config/model.rs` (`close_pane_if_idle: BindingConfig`, empty by default), `src/config/keybinds.rs` (action wiring), `src/client/shell/input.rs::close_focused_pane_if_idle` (returns true only when consumed; fails closed on no snapshot, no focused pane, or any agent entry for that pane), `src/input/keybind_help.rs` and `docs/next/website/src/data/config-reference.json`.

**Fidelity gap, deliberate.** Before v0.9.0 the check was the real one: the pane's foreground job is the pane's own shell, the same signal `agent.start` gates on. v0.9.0 moved key dispatch into the client, which has no live process tree, so the check is now "no agent entry in the cached snapshot" and a pane running `vim` is closeable. `tests/fork_contract.rs` keeps that gap as an explicitly `#[ignore]`d test. Closing it needs a server-side `pane.close_if_idle` method, which is its own task, not a client-side patch.

**Verify.** `fork_contract_close_pane_if_idle_*` - unbound by default, a bare alt chord accepted from config, an agent-free pane closed, a pane with an agent left alone. The decision logic lives entirely in `ClientShellState`, so the test drives `handle_input_bytes` directly rather than the socket.

**Replication hazard.** Upstream reshuffles `src/client/shell/input.rs` often; the call must stay at the top of direct-key handling, before generic forwarding, or the chord reaches the pane and never closes it.

---

## F3 - pane titles survive a live handoff

**Purpose.** After a live handoff every pane lost its sidebar name until the program inside emitted a new title. Nine jcode agents went nameless while their sessions stayed intact.

**How it works.** `src/app/mod.rs::new_from_handoff` collects every imported pane id and calls `sync_terminal_titles` once. That function normally only reads panes the parser marked dirty, and an imported pane is never dirty: its title came from an OSC sequence the *previous* server consumed, so the seeded value sat in the runtime with nothing to publish it into `TerminalState`.

**Verify.** `fork_contract_live_handoff_keeps_pane_terminal_titles` - sets a title with a real OSC 2 sequence, performs a real handoff, requires the title after. Delete the sync line and it fails with `left: None`, the exact observed defect.

---

## F4 - idle aging (stale and parked)

**Purpose.** One idle indicator could not tell "finished result you have not read" from "pane you already dealt with". With many agents that is the difference between a useful sidebar and wallpaper.

**Intent.** Four idle presentations from **two clocks**, because the halves ask different questions.

| Bucket | Clock | Meaning | Dots / Symbols | Label |
|---|---|---|---|---|
| FreshUnseen | terminal `state_entered_at` | result just landed, unread | `●` / `✓` | done |
| StaleUnseen | terminal `state_entered_at` | unread past threshold | `◉` / `!` | stale |
| FreshSeen | pane `seen_at` | looked at recently | `○` / `○` | idle |
| ParkedSeen | pane `seen_at` | deliberately parked | `◌` / `◌` | parked |

**How it works.**

- `src/terminal/state.rs` - private `state_entered_at: Instant` + getter, advancing only on a *real* transition, because detection re-reports the same state on every screen scan.
- `src/pane/state.rs` - `seen_at`, `stale_notified`, `mark_seen(now)`. Every `pane.seen = true` write routes through `mark_seen`. Re-focusing an already-seen pane is still a look, so the clock moves and the pane stays out of parked.
- `src/workspace/aggregate.rs` - `AggregateStatus { state, seen, aged_from }` and `pane_aged_from`, which picks the clock: `seen_at` when seen, `state_entered_at` when not.
- `src/ui/status.rs` - `IdleAge`, `idle_age_for(seen, elapsed, threshold)`, and the glyph / label / colour tables.
- `src/app/actions.rs` - `next_idle_age_expiry()` and `alert_stale_unread_panes_at()`. An unread result crossing the threshold raises the existing needs-attention notification at most once per idle episode (`stale_notified`), reusing `agent_notification_delivery`. Parking stays silent.
- `src/app/runtime.rs` - `idle_age_deadline` + `repaint_due_idle_age(now)`: one wake per crossing, no polling, nothing mutated.
- **The wire carries two ages, and the difference matters.** `state_age_seconds` is time in the agent state; `idle_age_seconds` is `pane_aged_from`'s answer, i.e. the two-clock rule. `src/app/creation.rs` fills both on `PaneInfo`; `src/app/agents.rs` and `src/server/client_shell.rs` put them on `AgentInfo` / `ClientShellAgent`. Both are additive and optional, so no `PROTOCOL_VERSION` bump. The glyph classifies from `idle_age_seconds`; classifying from `state_age_seconds` is the bug fixed in `ffe76393`, where a pane you had just looked at drew as parked and disagreed with the server's own alert.
- `src/client/shell/agent_sidebar.rs` - `render_agent_row` calls `idle_age_for` with the wire age and draws the bucket's glyph and label. This is the half the v0.9.0 port dropped; it was dead code until `a421888d`.
- `src/config/model.rs` - `ui.idle_stale_after_seconds`, default 300, plus `IDLE_STALE_CHOICES` (2/5/10/30 minutes); `src/client/shell/settings*.rs` - the "idle aging" settings section.

**Verify.** `fork_contract_state_entered_at_*`, `fork_contract_mark_seen_*`, `fork_contract_unread_result_alerts_once_*`, `fork_contract_idle_age_glyph_renders_from_wire_state_age_seconds`, `fork_contract_pane_reports_idle_age_seconds_from_the_look_clock` (a real pane, a real report, a real look, the real `render_agent_row`), `fork_contract_pane_info_reports_state_age_seconds_over_the_api`, and the settings-section test that drives real Tab keystrokes.

**Deadline must be in the future (2026-09-28).** `next_idle_age_expiry` (`src/app/actions.rs`) returns only crossings still ahead. Before, one idle pane already past the threshold pinned `idle_age_deadline` to that past instant: every main-loop pass found it due, `repaint_due_idle_age` recomputed the same instant, and the server never slept - 99.5% of a core on Joel's Mac, main thread busy in 4115 of 4116 samples. A fresh throwaway session hides it because no pane has aged yet; reproduce with a pane idle longer than `ui.idle_stale_after_seconds`. Test: `fork_contract_idle_age_deadline_never_lies_in_the_past`. Finding 0042 in `~/MYNE/Projects/tools/docs/findings/`.

**Still dead on purpose.** `src/ui/status.rs::idle_age_at` and `AgentPanelEntry.aged_from` keep `#[allow(dead_code)]`: they are the `AppState`-side equivalents, unused because the client classifies from the wire. Delete them or wire them, but do not assume their presence means the glyph path works - check `render_agent_row` instead.

---

## F5 - subagent nesting

**Agents-above-spaces reverted 2026-09-28.** Joel wants upstream's sidebar order back: spaces/machines on top, agents at the bottom, in both the single-endpoint sidebar and the multi-endpoint (machines) sidebar. `sidebar_section_heights`, `expanded_sidebar_sections` and `sidebar_section_divider_rect` in `src/ui/sidebar.rs` are reverted to upstream's exact code (`split_ratio` is the *workspace/spaces'* share again, section rects return `(workspace, detail)` = `(spaces, agents)` top to bottom). The footer-collision clamp (`.min(area.bottom() - 2)` on `footer_y` in `src/client/shell/sidebar.rs` and `endpoint_sidebar.rs`) existed only because spaces sat at the bottom; with spaces back on top it is dead weight and is reverted too, back to upstream's plain `workspace_area.bottom().saturating_sub(1)`. **Persisted `sidebar_section_split` note:** the field's meaning flips back (spaces' share, not agents'); an old saved ratio still lands inside the same `0.1..=0.9` clamp so it can never produce a broken or zero-height layout, just an inverted split for anyone who had manually dragged the divider under the old meaning. Joel's own `~/.config/herdr/session.json` carries `sidebar_section_split: null` (never manually resized), so this has no practical effect on his daily session; no migration was written for the general case - not worth it for a single-user cosmetic preference (YAGNI). Subagent nesting below is untouched by this revert.

**Purpose.** A coordinator's subagents belong under the coordinator, not scattered.

**How it works.**

- `src/ui.rs` - `SUBAGENT_PARENT_TOKEN = "parent_pane"`, a plain metadata token so nesting needs no wire, protocol or persistence change and a dead parent degrades to a flat row. Reported by the spawning agent: `herdr pane report-metadata <child> --source <id> --token parent_pane=<parent public pane id>`. herdr overwrites `HERDR_PANE_ID` in every pane it launches, so only the splitting parent can report the link.
- `src/client/shell/agent_sidebar.rs` - `nested_agent_pane_ids` returns `(pane_id, is_child)` and `ordered_agent_pane_ids` delegates to it, so hit map, keyboard navigation and drawn rows cannot disagree. Children keep incoming order, descendants flatten to one indent level, a cycle or missing parent stays flat, child rows draw `├─`/`└─`.
- `src/ui/sidebar.rs::nest_subagent_entries` - the same ordering on the server-side `AgentPanelEntry` list.

**Multi-endpoint nesting (more than one machine connected).** The single-endpoint path above only ever saw one endpoint's snapshot, so it was never the broken half. The federated path was: `endpoint_agents::agent_rows` built every row up front via `agent_sidebar::agent_row` with `nested` hardcoded to `false` (it also fed `AgentTokenContext.nested`, which changes which tokens a row shows, not just its indentation - a child row drops its Workspace/Tab tokens), and `aggregate_navigation::aggregate_agent_rows`, the one place that decides the cross-endpoint row order every consumer (drawn rows, the hit map, keyboard navigation) reads, did no parent grouping at all. So a subagent pane rendered as a flat sibling the moment a second machine connected, even though `herdr agent list` reported the correct `parent_pane` token on both machines. Fixed by:
  - `src/client/shell/agent_sidebar.rs::nest` - the tree walk extracted out of `nested_agent_pane_ids` into a generic `fn nest<K: Eq + Clone>(flat: Vec<K>, parent_of: impl Fn(&K) -> Option<K>) -> Vec<(K, bool)>`, so there is exactly one implementation of "children keep incoming order, descendants flatten to one indent level, a missing parent or a cycle stays flat" for both paths to share, not two copies to drift.
  - `src/client/shell/aggregate_navigation.rs::nest_endpoint_rows` - calls `nest` on `aggregate_agent_rows`'s already filtered/sorted output, keyed by row index so a row's `parent_pane` token only matches another row from the *same* endpoint - pane ids are unique per endpoint, not across all of them. Runs unconditionally at the end of `aggregate_agent_rows`, after whichever branch (a custom agent view, priority sort, or the plain per-endpoint order) produced the rows, the same way the single-endpoint path always nests after `flat_agent_pane_ids` regardless of sort mode. `AggregateAgentRow` grew a `nested: bool` field to carry the result.
  - `src/client/shell/agent_sidebar.rs::agent_row` and `src/client/shell/endpoint_agents.rs::agent_rows` - `agent_row` now takes `nested: bool` instead of hardcoding it, and the federated `agent_rows` computes the aggregate order (and its `nested` flags) *before* rendering any row content, then calls `agent_row` once per row in that order - it cannot build content first and reorder after the way the old two-phase hashmap lookup did, because `nested` changes what a row's tokens show.

**Verify.** `fork_contract_subagent_panes_render_indented_under_their_parent` asserts the single-endpoint drawn rows; `fork_contract_subagent_panes_nest_under_their_parent_with_two_endpoints` does the same with a second (SSH) endpoint connected, using an unrelated agent on the second endpoint that reuses the first endpoint's parent pane id to prove the parent lookup is scoped per endpoint, and also asserts `ClientShellState::online_agent_order_for_test` (a `#[cfg(test)]`-only accessor added for this, since `aggregate_navigation` is private outside `client::shell`) agrees with the drawn order. `fork_contract_sidebar_renders_spaces_header_above_agents_header` asserts the composed frame shows the spaces header above the agents header (renamed and flipped 2026-09-28 from the old agents-above-spaces assertion); the matching footer-clamp test was deleted since the clamp no longer exists.

**Observed.** 2026-09-28: Joel's own client, local plus the saved SSH machine `ssh-joel`, drew every subagent flat; `ssh-joel`'s `herdr agent list` did carry `parent_pane` tokens. After installing `860e418d` and reattaching (client started 18:53:57), Joel confirmed the children render nested under their parent. Diagnosis and proof: finding 0040, `2026-09-28-herdr-nesting-flat-with-two-machines.md` in `~/MYNE/Projects/tools/docs/findings/`.

**Replication hazard, the big one.** This feature was written twice, then broken a third way. v0.9.0 moved agent-row *rendering* into the client shell and kept only the half that computes the hierarchy, so panes rendered flat with nothing failing. Then the federated (multi-endpoint) sidebar path turned out to be a *third* implementation that had never grown nesting at all. After any upstream merge, check all three: the single-endpoint entry data, the single-endpoint row rendering, and the federated path's `aggregate_agent_rows`/`agent_row` wiring. The contract tests now cover all three, which is why none of them may ever move into an upstream-owned file.

---

## F6 - moved pane's old id survives a live handoff

**Purpose.** `pane.move` across workspaces gives a pane a new public id and
records `old -> new` in `App::state.public_pane_id_aliases`, so an agent
still reporting to its baked-in `HERDR_PANE_ID` keeps resolving. Before this
fix, a live handoff after such a move dropped the alias map: the old id came
back `pane_not_found` and the agent's every subsequent report was silently
unroutable. Observed live in a throwaway session with herdr 0.9.1: before
handoff `env=w1:p2 resolves="pane_id":"w2:p2"`, after handoff `env=w1:p2
resolves="code":"pane_not_found"`. Diagnosed and left deferred in finding
`2026-09-28-moved-pane-alias-lost-on-live-handoff.md` (0038) in
`~/MYNE/Projects/tools/docs/findings/`.

**How it works.**

- `src/server/handoff.rs::HandoffManifest` - `#[serde(default)] public_pane_id_aliases: HashMap<String, String>`, old public id -> the pane's current public id at export time, mirroring the `api_window_title` field right above it for old-manifest compatibility.
- `src/app/fork_pane_aliases.rs` (new fork-owned file) - `App::export_public_pane_id_aliases` (state's `PaneId`-keyed map -> public-id-keyed map, dropping dead aliases) and `App::import_public_pane_id_aliases` (public-id-keyed map -> state's `PaneId`-keyed map, skipping an alias whose `old` id already resolves on its own so a stale alias never shadows a real one). Public ids are the wire format because handoff import can renumber raw `PaneId`s; a stringly-keyed map survives that renumbering the same way `PaneId` itself cannot.
- `src/server/headless/lifecycle.rs` - one line after `manifest_for(...)`: `manifest.public_pane_id_aliases = self.app.export_public_pane_id_aliases();`.
- `src/server/headless/bootstrap.rs` - one line after `App::new_from_handoff(...)`: `app.import_public_pane_id_aliases(&received.manifest.public_pane_id_aliases);`.

**Verify.** `fork_contract_moved_pane_old_id_resolves_after_live_handoff` (`tests/fork_contract.rs`) drives a real cross-workspace move then a real live handoff and requires the old id still resolves. `fork_contract_a_manifest_written_before_public_pane_id_aliases_still_loads` (`src/fork_contract_tests.rs`) requires a manifest missing the field still deserialises, for a handoff between two herdr builds that straddle this change.

**Replication hazard.** Upstream owns `handoff.rs`, `lifecycle.rs`, and `bootstrap.rs`; after any merge, re-check that all three hook lines above still exist verbatim, since a conflict resolution that regenerates `manifest_for`'s call site or `new_from_handoff`'s binding can silently drop them without a compile error (`manifest.public_pane_id_aliases` defaults to empty, `app.import_public_pane_id_aliases` not being called just means the map stays empty - both compile fine and only `fork_contract_moved_pane_old_id_resolves_after_live_handoff` catches it).

---

## Fork-owned deviation: the frozen endpoint-shape fixture

`tests/fixtures/endpoint-method-shapes-v1.json` is a compatibility contract, and upstream's rule is never to re-bless a generation-1 expectation. The fork's `IntegrationTarget::Jcode` variant changes the `integration.install` shape digest, so the test failed permanently. In this fork our binary is both client and server, so the fork re-blesses its own copy, with the reasoning at the assertion site in `src/server/client_commands.rs`.

Two consequences a future port must honour: never carry this re-bless upstream, and re-generate it again after re-adding `Jcode` on the next merge. It also means a fork-built client talking to an upstream-built server is out of contract on that method - we do not do that, and `IntegrationTarget` has no `Unknown` fallback to make it safe if we ever did.

---

## F7 - accept moved off the main loop (JSB-17)

**Purpose.** The headless server main loop burned CPU and answered client input late. `docs/next/known-issues/2026-09-28-main-loop-cpu-spin.md` diagnosed the first half (a non-blocking `accept()` called on every pass) and half of the second (F4's idle-age repaint deadline, ruled out below). This closes JSB-17.

**What was actually happening, measured 2026-09-28 with `HERDR_RENDER_PROF=1` on a throwaway session (`.agents/skills/herdr-throwaway-repro/SKILL.md`), never Joel's own session.** The known-issues doc's own "idle server spins at max speed" framing does not hold on this build: a truly idle server (no panes producing output, no API traffic) already woke only ~5-6 times/second, capped correctly by `CLIENT_ACCEPT_POLL_INTERVAL` (250ms). The real cost showed up under load that looks nothing like raw byte volume: six panes each writing a few bytes every 200ms (an agent spinner's shape, not a stress test) drove the main loop to ~73-77 passes/second. `render_prof` counters (`loop.wake.*`, added in this fix) broke that down: `render_notify` wakes tracked real PTY output 1:1 (correct), but `ServerEvent::ClientWriterDrained` - sent once per render frame handed to the client-writer thread purely to release backpressure - fired on *another* ~14-16 passes/second that did no new work at all. Every one of those ~73-77 passes still called `accept_client_connections()` unconditionally, so ~73-77 real `accept()` syscalls/second returned `EWOULDBLOCK` for nothing. On a quiet Mac each such syscall costs low single-digit microseconds and the effect is invisible in `ps` CPU time; the known-issues doc's own `strace` capture on Joel's real (CPU-saturated, load 6-8 on 8 cores) Linux box measured 300-800us per syscall under contention, which is where this became visible as burned CPU and late input.

**F4 idle-age is ruled out, not fixed here.** The known-issues doc named `repaint_due_idle_age`/`sync_idle_age_deadline` (`src/app/runtime.rs`) as a second suspect. Measurement did not support it: `next_idle_age_expiry` only returns a near-term deadline when a pane is actually close to crossing the stale/parked threshold (minutes, from `ui.idle_stale_after_seconds`), not on every pass, and the idle-age counters never dominated a profiler window in either the idle or the busy-render capture. If a future capture shows otherwise, treat it as a new finding, not a re-opening of this one.

**The fix, in preference order from the task.** Accept moved off the main loop entirely, mirroring the Windows path (`spawn_windows_client_accept_thread`, unaffected by this change) rather than throttling readlinks or patching a spurious wake source, because the wake sources above are legitimate (they still fire after this fix) and the waste was specifically the accept4 call riding along on all of them for free.

- `src/server/client_accept.rs::spawn_unix_client_accept_thread` - a dedicated thread that blocks in `poll(2)` (`wait_for_listener_readable`, using the already-a-dependency `libc` crate, not a new one) on the listener's raw fd with a 250ms timeout (to recheck `should_quit` on the same cadence the old poll interval offered), and only calls `accept_pending_client_connections` / `reject_pending_client_connections` - both unchanged - when the fd actually reports readable. The listener stays non-blocking (`ListenerNonblockingMode::Accept`) so those two functions keep their exact `WouldBlock`-terminated batch-drain behavior; only *when* they run changed, not what they do.
- `HeadlessServer::accept_client_connections` (both the unix polling variant and the windows no-op) is deleted outright, along with its call in the main loop's "4. Accept new client connections" step. Windows already accepted off the main loop; unix now does too, so there is nothing left for the main loop to do here on either platform.
- **Handoff/reject semantics, preserved exactly.** The old code read `self.handoff_in_progress: bool` (main-thread-only) inside `accept_client_connections` to decide accept vs. reject; the accept thread cannot read that field, so a new `handoff_reject_new_clients: Arc<AtomicBool>` field mirrors it, written at the same three call sites `handoff_in_progress` already was (`src/server/headless/lifecycle.rs`: entering handoff, the early `bind_listener` failure, and `rollback_handoff_before_commit`). The old code additionally did one *synchronous* `reject_pending_client_connections` call at the instant handoff began, to flush anything already queued before the next main-loop pass got to it; that call is deleted as redundant, since the accept thread now polls independently of the main loop's pace and will reject on its own next wake (within the same 250ms bound) with no dependency on the main loop reaching anything.
- `restore_public_sockets_after_failed_handoff` (the rare path where a handoff attempt fails and the old server must resume public service) used to store the freshly bound listener back into `self.client_listener` for the main loop to keep polling. There is no more `client_listener` field to store it in; it spawns a fresh `spawn_unix_client_accept_thread` for the new listener instead. The old thread (if any) exits on its own the next time its now-invalid fd errors out of `poll(2)` - the same "dies when its fd goes bad" pattern already used elsewhere for background IO threads, no explicit stop signal needed.
- `next_client_id: u64` moved from a `HeadlessServer` field to a local inside the accept thread (mirroring how `spawn_windows_client_accept_thread` already owned its own counter); nothing outside the thread ever read it.

**Measured before/after, same throwaway session, same six-panes-at-200ms load, `HERDR_RENDER_PROF=1`:** `loop.tick` ~73-77/s -> ~25/s; `accept.attempt` (new counter, fires on every real call to `accept_pending_client_connections` regardless of caller) ~82-125/s -> 0 (never printed - `render_prof::counter` skips zero values). `ps -o time=` CPU-seconds over a 60s window on this quiet, uncontended Mac showed no measurable difference (~0.4% of a core both before and after) - expected, since the syscalls this removes are cheap when uncontended; the payoff is in the syscall *count*, which is what turns into real CPU under the contention the known-issues doc measured on Joel's box, not in an idle Mac's `ps` output. See `docs/next/known-issues/2026-09-28-main-loop-cpu-spin.md` for the corrected baseline this entry supersedes.

**Confirmed under real contention on `ssh-joel`** (load 6-8 on 8 cores, the box the known-issues doc's original capture came from), same throwaway-session method, `c417b6da` vs this branch's `ac96d43c`, both built there: `strace -qq -c -p <main thread>` for 5s went from `accept4` 237 calls/errors=237 (14.7% of traced syscall time) to **zero `accept4` calls at all** - not reduced, gone from the main thread's syscall profile, because it now runs on the dedicated accept thread instead. `ps -o time=` CPU-seconds over the same 30s window: ~7s (23% of a core) before, ~4s (13% of a core) after. `futex` stayed the dominant cost on both sides (this fix does not touch it - see the F4 ruled-out note above), so this is not a full fix for the box's overall CPU picture, only for the accept-spin half of it named in the known-issues doc's title.

(Throwaway binaries and session artifacts from the `ssh-joel` measurement were left at `/root/herdr-baseline`, `/root/herdr-fixed` and `/root/jsb17-remote/` - harmless, not installed, no process left running - because this session's `rm` guard blocks absolute-path deletes on a remote host the same as a local one; delete them by hand or leave them as reference.)

**Verify.** `fork_contract_busy_render_loop_does_not_drive_per_pass_accept_calls` (`tests/fork_contract.rs`) drives a real spawned server with one real attached client and one real pane in a busy write loop, then asserts from the real `herdr-server.log` that `loop.tick` was clearly elevated (proving the busy-render scenario actually happened) while `accept.attempt` stayed near zero (proving accept did not ride along on those passes). Reverting just this fix (keeping the counter) reproduces `loop.tick=83 accept.attempt=82` and fails the assertion; this is the exact RED observed while developing the fix, kept here as the regression signature to look for if this is ever undone.

**Replication hazard.** This fix touches upstream-owned `src/server/headless.rs`, `src/server/client_accept.rs` and `src/server/headless/lifecycle.rs`. On the next upstream merge: **check whether upstream fixed this differently first** - `accept_client_connections`'s unix/windows split and the `CLIENT_ACCEPT_POLL_INTERVAL` main-loop poll are exactly the kind of thing upstream might solve on their own (they already solved it for Windows). If upstream ships its own fix, prefer it and drop this patch rather than carrying two solutions to the same problem; if not, re-apply this same shape (dedicated poll-gated accept thread, `handoff_reject_new_clients` atomic mirroring `handoff_in_progress`) after resolving whatever the merge conflict looks like. If `accept_client_connections` or `next_client_id` come back from a conflict resolution that took upstream's side wholesale, that is the sign this patch was silently dropped rather than merged - `fork_contract_busy_render_loop_does_not_drive_per_pass_accept_calls` will not go red on its own for this, since the function reappearing does not by itself fail the test until something actually drives a busy render loop against it (which the test does), so treat any full-file revert of `headless.rs` as a reason to re-check this section by hand, not just by running the suite.

---

## Fork-local docs and rules

- Diagnosed herdr bugs live in `~/MYNE/Projects/tools/docs/findings/` (index `README.md` there), never in this repo: `2026-09-28-moved-pane-alias-lost-on-live-handoff.md` (0038, stale `HERDR_PANE_ID`, fixed by F6) and `2026-08-28-suppression-latch-drops-agent-reports.md` (0039, drop reason now observable - fixed 2026-09-28, the latch's real exit is still open). Read both before confusing the two. Finding 0039 applies to every full-lifecycle-hook-authority source, not only the now-retired jcode (F1) - see `src/detect/mod.rs::full_lifecycle_hook_authority`, which also covers `herdr:pi`. As of the 2026-09-28 fix, a dropped report's reason is visible over `agent.explain`'s `skipped_update_reason` and as one `tracing::info!` line per drop in `herdr-server.log` (`src/terminal/fork_report_drops.rs`).
- `docs/next/known-issues/2026-09-28-main-loop-cpu-spin.md` - the server main loop spun on a non-blocking `accept()` called on every pass. Fixed by F7 (JSB-17): accept moved to a dedicated poll-gated thread, mirroring the existing Windows path. F4 idle-age was ruled out, not fixed - see F7 for why. Read F7 before touching `src/server/client_accept.rs`, `src/server/headless/` or `repaint_due_idle_age` again.
- `docs/findings/2026-09-27-debug-vt-lib-burns-a-core.md` - the installed binary built with the vt lib at `Debug` burned 64% of a core and queued every keystroke behind a page integrity check. Read it before rebuilding or reinstalling `~/.local/bin/herdr`.
- `AGENTS.md` - the "installing a tool or plugin" rule and the pointer to this file.
- `.agents/skills/herdr-throwaway-repro/SKILL.md` - `-u HERDR_ENV` is required in the launch command, or the nested session refuses to start.
- Code comments in `src/ui/sidebar.rs` still point at `.local/PORT-0.9.0.md`, which is gitignored and no longer on disk. Dangling; this file carries what mattered.

---

## Build environment on this Mac

Two overrides, both required, neither optional since the 2026-09-27 port:

```bash
export PATH=/var/tmp/xcrun-shim:$PATH      # zig cannot use the CommandLineTools 26 SDK
export ZIG=/opt/homebrew/bin/zig           # upstream now requires Zig 0.16.0
```

- **The SDK shim.** zig links `aarch64-macos`; the CommandLineTools 26.x SDK dropped plain `arm64-macos` from `libSystem.tbd`, so every libc symbol comes back undefined and the build dies in `build.rs`. `SDKROOT` does not help - zig shells out to `xcrun --show-sdk-path` and ignores the variable. Recreate the shim when `/var/tmp` is cleared:

```bash
mkdir -p /var/tmp/xcrun-shim
SDK=/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk
cat > /var/tmp/xcrun-shim/xcrun <<EOF
#!/bin/sh
for a in "\$@"; do
  case "\$a" in
    --show-sdk-path) echo "$SDK"; exit 0;;
    --show-sdk-version) echo "15.4"; exit 0;;
  esac
done
exec /usr/bin/xcrun "\$@"
EOF
chmod +x /var/tmp/xcrun-shim/xcrun
```

- **Zig 0.16.0.** Upstream `fff6c820` extracted the libghostty-vt binding into `crates/ghostty-vt` and raised the vendored minimum from 0.15.2 to 0.16.0. The zig on `PATH` here is still 0.15.2 and `build.rs` panics with `Building Herdr requires Zig 0.16.0`; Homebrew's 0.16.0 at `/opt/homebrew/bin/zig` satisfies it through the `ZIG` env var, with no patch to `build.rs`.

`cargo fmt --check` and parts of `cargo nextest run` pass without either override, which is what makes a missing shim confusing. The maintenance Python suites need python3.12 (`tomllib`); system `python3` is 3.9.

### The binary you install must have the vt lib at ReleaseFast

`build.rs:54` defaults `LIBGHOSTTY_VT_OPTIMIZE` to `ReleaseFast`, independently of the cargo profile - a cargo `--debug` build still gets a correct vt lib. Override it to `Debug` and the binary is unusable as a daily driver: `Screen.clearCells` then calls `Page.verifyIntegrity` on every `erase line`, which walks the whole page and builds two hashmaps while holding the pane's terminal mutex. Measured on the Sep 10 build: **64% of a core sustained** and every keystroke queued behind the check. Full diagnosis in `docs/findings/2026-09-27-debug-vt-lib-burns-a-core.md`.

Check any binary on disk before installing it. No running server and no load needed:

```bash
otool -tvV ~/.local/bin/herdr \
  | awk '/^_?terminal\.Screen\.clearCells/{f=1} f&&/^[a-zA-Z_]/&&!/clearCells/{f=0} f&&/bl\t/{print $NF}' \
  | sort -u | grep -ci integrity
```

`0` is a good binary. `2` is the bug, whatever the version string says. Do not verify this with `sample` on a running server: an idle server, or the wrong PID out of several, answers `0` regardless.

---

## Replication procedure onto a newer upstream

1. `git fetch upstream && git checkout -b port/<date> master`.
2. `git merge upstream/master`. Expect the conflicts to cluster exactly where upstream adds its own integrations and agents: `src/integration/*`, `src/detect/mod.rs`, `src/cli/integration.rs`, `src/agent_resume.rs`, `src/client/shell/agent_sidebar.rs`, and the translated `integrations.mdx` tables.
3. Resolve by taking upstream's structure and re-adding our entry - **except jcode (F1), retired 2026-09-28: for every jcode-related hunk in those files, take upstream's side and let the jcode code disappear, do not re-add it.** Never keep our copy of a file upstream moved or rewrote. Then fix the array-size annotations the compiler flags.
4. `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
5. **`cargo nextest run fork_contract`.** This is the whole point of the file: a green run means every feature above survived, and a red one names which did not. Do not go exploring the features by hand first.
6. Full `cargo nextest run`, compared against the numbers in Ground truth. The frozen endpoint-shape fixture needs re-blessing again (see above).
7. Update Ground truth and the commit map here, in the same commit.
8. A fork feature you had to re-implement differently gets its section rewritten here and its contract test strengthened in the same commit - a test that could not catch the loss you just repaired is the next port's silent failure.
