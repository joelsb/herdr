# Fork features (joelsb/herdr)

What this fork adds on top of upstream `herdrdev/herdr`, why each thing exists, how it actually works, and what a future agent must redo when rebasing or re-merging onto a newer upstream.
Fork-local file. Upstream has no `FORK.md`, so it never conflicts.

## Ground truth

| Fact | Value |
|---|---|
| origin | https://github.com/joelsb/herdr (our fork, branch `master`) |
| upstream | https://github.com/herdrdev/herdr |
| upstream point we sit on | `1682cab3` `fix: restore plugin focus events for client navigation (#3850)`, 2026-09-09, 9 commits after tag `v0.9.0` |
| our commits on top | 17 (`git log --oneline 1682cab3..master`) |
| our diff | ~3.7k added lines outside `vendor/` |
| verified | 2026-09-27, by reading this repo, not from memory |

Recompute the base any time: `git merge-base master upstream/master`.
List our work: `git log --oneline $(git merge-base master upstream/master)..master`.
List our files: `git diff --stat $(git merge-base master upstream/master)..master -- . ':(exclude)vendor'`.

## Commit map

| Commit | Feature |
|---|---|
| `46d8d3f3` | F1 jcode integration (hook asset, config editing, registry, detect gating) |
| `e87abeb4` | F1 session anchoring before the first state report + `scripts/verify_jcode_hook.py` |
| `b284249d` | F1 re-anchor on `jcode --resume` |
| `04278c88` | F2 `close_pane_if_idle` keybinding |
| `61587c39` | F3 pane titles survive a live handoff |
| `d6b82f42` `c54e7920` `276e8c91` `e5d4df50` | F4 idle aging (buckets, settings, API field, docs) |
| `9611e6b7` | F5 agents section above spaces + subagent nesting (server-side data) |
| `b94e4ffd` `e39543ce` `18794fbb` | v0.9.0 merge, compile/test repair, nesting re-done client-side |
| `6c14fb80` `f000439b` `d6b6a1bc` | docs: known issues, tools-checkout rule, throwaway-repro fix |

---

## F1 - jcode integration

**Purpose.** jcode ran in herdr panes with `agent_status=unknown` and never appeared in the Agents panel, so a wall of jcode agents was invisible to herdr.

**Intent.** Make jcode a *full lifecycle authority*: hooks own the state, no screen detection, and the session id is persisted so a server restart relaunches `jcode --resume <id>` instead of a bare shell.

**How it works.**

- `src/integration/assets/jcode/herdr-agent-state.sh` - the installed reporter, `/bin/sh` + inline `python3`. One script for all five events; it reads `JCODE_HOOK_EVENT` because jcode passes hook metadata in env vars and writes nothing to stdin.
  - Event to state: `session_start` = idle, `turn_start` and `post_tool` = working, `turn_end` = idle when `JCODE_HOOK_STATUS=ok` else blocked, `session_end` = `pane.release_agent`.
  - `turn_start` exists in jcode and fires before the model streams, so a turn that only thinks still reports working. That is why jcode is a lifecycle authority and not a session-only integration.
  - Guards, all exit 0 silently: `HERDR_ENV=1`, `HERDR_SOCKET_PATH`, `HERDR_PANE_ID`, `python3` on PATH, event in the known list.
  - Swarm guard: a visible swarm worker pane runs this hook too and already reports for itself. The script reads `$JCODE_HOME/sessions/<id>.json` and skips when `parent_id` is set.
  - `seq = time.time_ns()`. herdr keeps the last seq per `(pane, source)` and drops anything not greater, so a per-session counter would be ignored for the life of the pane.
  - On `session_start` it sends `pane.report_agent_session` first (seq N) and then `pane.report_agent` (seq N+1). Without the anchor, `route_full_lifecycle_hook_report` answers `ok` and discards the state - verified live 2026-08-24.
- `src/integration/mod.rs` - `JCODE_HOOK_ASSET`, `JCODE_HOOK_INSTALL_NAME`, `JCODE_HOOK_EVENTS`, `JCODE_INTEGRATION_VERSION = 1`. Unix only, no PowerShell asset.
- `src/integration/env.rs` - `jcode_dir()`: `$JCODE_HOME` else `~/.jcode`.
- `src/integration/targets.rs` - `install_jcode` / `uninstall_jcode`: write `~/.jcode/hooks/herdr-agent-state.sh`, chmod +x, then edit `~/.jcode/config.toml`.
- `src/integration/config_edit.rs` - `build_jcode_config_with_hooks` / `remove_jcode_config_hooks`. jcode keeps hooks in `[hooks]` and accepts one command or an array per event, so install *appends* and uninstall removes only our command; a user dispatcher already there keeps working.
- `src/integration/registry.rs`, `actions.rs`, `types.rs`, `src/cli/integration.rs`, `src/api/schema/integrations.rs` - the target `Jcode`, arity bumps `17 -> 18`, CLI strings.
- `src/detect/mod.rs` - `Agent::Jcode` added to `ALL` (`23 -> 24`) and to `full_lifecycle_hook_authority`, deliberately **not** in `SCREEN_MANIFEST_AGENTS`: two authorities on one pane is the bug this avoids.
- `src/agent_resume.rs` + `src/persist/restore.rs` - `herdr:jcode` is an official source, so a snapshot restores as `jcode --resume <id>`. An unofficial source reporting the same agent is refused.
- `src/terminal/state.rs` - `("herdr:jcode", "jcode", Some("resume" | "new"))` allowed as a session *replacement*. `jcode --resume <id>` fires `session_start` twice (`new` then `resume`); without replacement the pane stays anchored to the throwaway id and every later report is dropped.

**Verify.**

```bash
cargo nextest run jcode                                              # rust side
python3 scripts/verify_jcode_hook.py src/integration/assets/jcode/herdr-agent-state.sh
```

`verify_jcode_hook.py` runs the real script against a stand-in Unix socket and asserts the actual JSON-RPC on the wire, which no Rust test covers.
Live check: `herdr integration install jcode`, start jcode in a pane, watch the sidebar go working/idle.

**Replication hazards.**

- Every upstream release adds integration targets, so `IntegrationTarget::ALL` and `integration_specs()` arity constants conflict on almost every merge. Resolve by taking upstream's list and re-adding `Jcode` at the end.
- Same for `Agent::ALL` in `src/detect/mod.rs`.
- Keep `Jcode` out of `SCREEN_MANIFEST_AGENTS`. If a future upstream adds a bundled `jcode.toml` manifest, drop ours or drop the hook authority; never both.
- `HERDR_INTEGRATION_VERSION=1` in the asset must match `JCODE_INTEGRATION_VERSION`; upstream's maintenance test checks that pairing.

---

## F2 - `close_pane_if_idle`

**Purpose.** One bare chord that first reaches the agent and then closes the pane. `alt+x` exits jcode; press it again and the now-idle pane closes.

**Intent.** A plain `close_pane` on a bare chord would kill a pane while the agent still runs. This binding only acts when the pane is free, otherwise the key is forwarded untouched.

**How it works.**

- `src/config/model.rs` - `close_pane_if_idle: BindingConfig`, empty by default (strong opt-in).
- `src/config/keybinds.rs` - action wiring.
- `src/client/shell/input.rs` - `close_focused_pane_if_idle()`. Returns true only when consumed; fails closed on no snapshot, no focused pane, or any agent entry for the pane.
- `src/input/keybind_help.rs`, `docs/next/website/src/data/config-reference.json` - help and reference entries.

**Fidelity note, load-bearing.** Before v0.9.0 the check was the real one: the pane's foreground job is the pane's own shell, the same signal `agent.start` gates on. v0.9.0 moved key dispatch into the client, which has no live process tree, so the current check is "no agent entry in the cached snapshot". A non-agent foreground program (`vim`, `less`) is now closeable. `tests/close_pane_if_idle.rs::alt_x_reaches_a_busy_pane_then_closes_it_once_idle` is `#[ignore]`d proving exactly that gap. Restoring full fidelity needs a server-side `pane.close_if_idle` method, not a client-side hack.

**Verify.** `cargo nextest run close_pane_if_idle` (unit tests in `src/input/close_pane_if_idle_tests.rs` pass; the ignored integration test is the documented gap).

**Replication hazards.** Upstream regularly reshuffles `src/client/shell/input.rs`; re-apply the call at the top of direct-key handling, before generic forwarding, or the chord will reach the pane and never close it.

---

## F3 - pane titles survive a live handoff

**Purpose.** After a live handoff every pane lost its sidebar name until the program inside emitted a new title. Seen live 2026-08-27: nine jcode agents went nameless while their sessions stayed intact.

**How it works.** `src/app/mod.rs::new_from_handoff` collects every imported pane id and calls `app.sync_terminal_titles(&imported_panes)` once. `sync_terminal_titles` normally only reads panes the parser marked dirty; an imported pane is never dirty, because its title came from an OSC sequence the *previous* server consumed, so the seeded value sat in the runtime with nothing to publish it into `TerminalState`.

**Verify.** `tests/close_pane_if_idle.rs::live_handoff_keeps_pane_terminal_titles` - sets a title with a real OSC 2 sequence, performs a real handoff, requires the title after. Delete the sync line and it fails with `left: None`, the exact observed defect.

**Replication hazards.** Small and self-contained. Upstream may fix this itself; check whether `new_from_handoff` already syncs titles before re-applying.

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

- `src/terminal/state.rs` - private `state_entered_at: Instant` + getter. Only advances on a *real* transition, because detection re-reports the same state on every screen scan. Reset by `clear_agent_runtime_identity_after_respawn`.
- `src/pane/state.rs` - `seen_at`, `stale_notified`, and `mark_seen(now)`. Every `pane.seen = true` write routes through `mark_seen` so the two fields cannot drift. Re-focusing an already-seen pane is still a look, so the clock moves and the pane stays out of parked.
- `src/workspace/aggregate.rs` - `aggregate_state` returns a named `AggregateStatus { state, seen, aged_from }` instead of a tuple; `pane_aged_from` picks the right clock.
- `src/ui/status.rs` - `IdleAge` enum, `idle_age_for(seen, elapsed, threshold)`, and the glyph / label / colour tables. `IdleAge` replaces a bare `seen: bool` at the presentation boundary, which makes "seen but aged from result time" unrepresentable.
- `src/app/actions.rs` - `next_idle_age_expiry()` and `alert_stale_unread_panes_at()`. An unread result crossing the threshold raises the existing needs-attention notification at most once per idle episode (`stale_notified`), reusing `agent_notification_delivery` so active-tab suppression, sound policy and the agent-identity check still apply. Going parked stays silent: parking is deliberate.
- `src/app/runtime.rs` - `idle_age_deadline` + `repaint_due_idle_age(now)`. Timer wakes the loop once per crossing instead of polling; nothing mutates, the bucket is derived from the clock at render time.
- `src/api/schema/panes.rs` + `src/app/api/panes.rs` - `PaneInfo.state_age_seconds`, optional. Elapsed seconds, not a timestamp, because another process cannot interpret this one's monotonic clock. The bucket itself is deliberately not sent: the threshold is client policy. Additive, so no `PROTOCOL_VERSION` bump.
- `src/config/model.rs` - `ui.idle_stale_after_seconds`, default 300, plus `IDLE_STALE_CHOICES` (2/5/10/30 minutes).
- `src/client/shell/settings.rs` + `settings_overlay.rs` - the "idle aging" settings section, built on the existing modal-choice-list pattern. A config value outside the list selects the default row.

**Verify.**

```bash
cargo nextest run idle          # actions.rs, status.rs, state.rs, pane/state.rs tests
```

Named tests worth keeping green: `state_entered_at_tracks_only_real_state_changes`, `mark_seen_advances_the_look_clock_every_time`, `an_unread_result_alerts_once_when_it_goes_stale`, `a_pane_the_user_looked_at_never_alerts_on_going_parked`, `idle_age_deadline_uses_the_look_clock_once_seen_and_a_look_pushes_it_out`, `pane_info_reports_how_long_the_agent_state_has_held`.
Live check that actually proves the feature: a throwaway session with `HERDR_CONFIG_PATH` set to a 60s threshold, drive a pane with `herdr pane report-agent`, watch `✓ -> ! -> ○ -> ◌`.

**KNOWN GAP after the v0.9.0 port.** The glyph/colour half is *not reachable*: `idle_age_at`, `state_icon`, `state_icon_symbol`, `state_label`, `state_label_color` all carry `#[allow(dead_code)]`, and `AgentPanelEntry.aged_from` is populated but unread. v0.9.0 moved sidebar row rendering into the client shell and the client snapshot has no elapsed-time field. Data, config, settings UI, API field and notification all work; the visual bucket does not. Finishing it means putting the age (or the bucket) in the client snapshot and calling `idle_age_for` where the agent rows are drawn (`src/client/shell/agent_sidebar.rs`).

**Replication hazards.** `aggregate_state`'s return type change touches several callers; expect conflicts wherever upstream destructures it as a tuple.

---

## F5 - agents above spaces, subagent nesting

**Purpose.** The agents list is what gets looked at; it belongs on top. A coordinator's subagents belong under the coordinator, not scattered.

**How it works.**

- `src/ui/sidebar.rs` - `sidebar_section_heights` returns `(top, bottom)` and `split_ratio` now means the *agents'* share; section rects return `(spaces, agents)` because callers address by role. The agents header dropped from three rows to two; the divider moved into the spaces header.
- Footer collision, load-bearing: the sidebar's collapse-toggle glyph always renders at the sidebar's last row. With spaces at the bottom, its `new`/`menu` footer collided with the toggle, so `footer_y` is clamped with `.min(area.bottom() - 2)` in both the single-machine and federated renderers (`src/client/shell/sidebar.rs`, `endpoint_sidebar.rs`).
- `src/ui.rs` - `SUBAGENT_PARENT_TOKEN = "parent_pane"`. A plain metadata token, not pane state and not an API field, so nesting needs no wire, protocol or persistence change and a dead parent degrades to a flat row instead of an invalid reference. Reported by the spawning agent: `herdr pane report-metadata <child> --source <id> --token parent_pane=<parent public pane id>`. herdr overwrites `HERDR_PANE_ID` in every pane it launches, so only the splitting parent can report the link.
- `src/client/shell/agent_sidebar.rs` - `nested_agent_pane_ids(snapshot, sort)` returns `(pane_id, is_child)`; `ordered_agent_pane_ids` delegates to it so the hit map, keyboard navigation and drawn rows cannot disagree. Children keep incoming order, descendants flatten to one indent level, a cycle or missing parent stays flat. Child rows draw `├─`/`└─` and name the pane instead of repeating the parent's workspace and tab.
- `src/ui/sidebar.rs::nest_subagent_entries` - the same ordering on the server-side `AgentPanelEntry` list (still used by the pure-data path).

**Verify.** `cargo nextest run reported_subagent_panes_render_indented_under_their_parent` plus the sidebar render tests.

**Replication hazards, the big one.** This feature was written twice. `9611e6b7` did it in the server-side sidebar; v0.9.0 moved agent-row *rendering* into the client shell and silently kept only the half that computes the hierarchy, so panes rendered flat until `18794fbb` redid the drawing in `src/client/shell/agent_sidebar.rs`. After any upstream merge, check both halves: the entry data **and** the row rendering. Nesting computed but flat on screen is exactly the failure mode that already happened once.
Second hazard: any test with a hardcoded sidebar coordinate breaks from the reorder. Three broke last time, two fixed, one still `#[ignore]`d (`tests/client_mode.rs::federated_client_starts_without_local_and_survives_its_restart`).

---

## F6 - fork-local docs and rules

- `docs/next/known-issues/stale-herdr-pane-id-agent-status.md` - a pane's public id changes but the agent's baked-in `HERDR_PANE_ID` does not, so it reports to a pane it no longer occupies. Diagnosed 2026-08-27, fix deferred by Joel.
- `docs/next/known-issues/pane-silently-rejects-agent-reports.md` - untracked in git, a suppression latch rejecting every report on one pane with a correct address. Read it before confusing the two.
- `AGENTS.md` "Local machine: installing a tool or plugin" - any installed tool or plugin also gets a `~/MYNE/Projects/tools/<name>/` checkout, a `.gitignore` entry and a `TOOLS.md` row.
- `.agents/skills/herdr-throwaway-repro/SKILL.md` - `-u HERDR_ENV` is required in the launch command; 0.9.0's `should_block_nested_for_env` refuses a nested session otherwise.
- `.gitignore` - `/.build-shim/`.

---

## Build environment on this Mac

zig 0.15.2 (pinned by the vendored libghostty-vt) links `aarch64-macos`, and the Command Line Tools 26.x SDK dropped plain `arm64-macos` from `libSystem.tbd`, so every libc symbol comes back undefined and `cargo check|clippy|build` dies at `build.rs`. `SDKROOT` does not help: zig shells out to `xcrun --show-sdk-path` and ignores the variable.

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
PATH=/var/tmp/xcrun-shim:$PATH just check
```

`cargo fmt --check` and `cargo nextest run` pass without the shim, which is what makes this confusing. Verified 2026-08-28.

Also: several code comments reference `.local/PORT-0.9.0.md` for the v0.9.0 resolution notes. That file is gitignored and **no longer on disk** (only `.local/prd/` survives). Treat those pointers as dangling; this file carries what mattered.

---

## Replication procedure onto a newer upstream

1. `git fetch upstream && git checkout -b port/<version> master`.
2. `git merge upstream/master` (or the release tag). Expect conflicts concentrated in: integration registry arity, `src/detect/mod.rs` agent lists, `src/client/shell/*` (upstream moves UI code between server and client), sidebar layout, and `src/app/actions.rs`.
3. Resolve by taking upstream's structure and re-adding our entry, never by keeping our copy of a moved file.
4. After the merge compiles, walk this file top to bottom and prove each feature still reachable - not just present. The 0.9.0 port passed `cargo fmt --check` with 62 compile errors and shipped a feature whose data was computed and never drawn.
5. Run `PATH=/var/tmp/xcrun-shim:$PATH just check`, then the per-feature verifications above, then `python3 scripts/verify_jcode_hook.py src/integration/assets/jcode/herdr-agent-state.sh`.
6. Update the "Ground truth" table and the commit map in this file in the same commit.
