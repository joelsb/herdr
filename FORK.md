# Fork features (joelsb/herdr)

What this fork adds on top of upstream `herdrdev/herdr`, why each thing exists, how it actually works, and what a future agent must redo when merging a newer upstream.
Fork-local file. Upstream has no `FORK.md`, so it never conflicts.

## Run this first

```bash
export PATH=/var/tmp/xcrun-shim:$PATH      # see Build environment
export ZIG=/opt/homebrew/bin/zig           # Zig 0.16.0; PATH zig is 0.15.2 and too old
cargo nextest run fork_contract            # 19 tests + 1 deliberately ignored
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
| `46d8d3f3` | F1 jcode integration (hook asset, config editing, registry, detect gating) |
| `e87abeb4` | F1 session anchoring before the first state report + `scripts/verify_jcode_hook.py` |
| `b284249d` | F1 re-anchor on `jcode --resume` |
| `04278c88` | F2 `close_pane_if_idle` keybinding |
| `61587c39` | F3 pane titles survive a live handoff |
| `d6b82f42` `c54e7920` `276e8c91` `e5d4df50` | F4 idle aging (buckets, settings, API field, docs) |
| `9611e6b7` | F5 agents above spaces + subagent nesting (server-side data) |
| `b94e4ffd` `e39543ce` `18794fbb` | v0.9.0 merge, compile/test repair, nesting re-done client-side |
| `bafc4c59` `2f41a6bc` | 2026-09-27 port: merge of upstream `fff6c820`, 11 conflicts, merge fallout |
| `a421888d` | F4 glyphs finally rendered from the client snapshot |
| `ffe76393` | F4 a seen pane ages from the look clock on the wire too |
| `b6d876ba` | fork-owned rebless of the frozen endpoint-shape fixture |
| `b9566910` `fa6d5fab` `a830597f` `eb1e9149` | the fork contract test set |

---

## F1 - jcode integration

**Purpose.** jcode ran in herdr panes with `agent_status=unknown` and never appeared in the Agents panel, so a wall of jcode agents was invisible to herdr.

**Intent.** Make jcode a *full lifecycle authority*: hooks own the state, no screen detection, and the session id is persisted so a server restart relaunches `jcode --resume <id>` instead of a bare shell.

**How it works.**

- `src/integration/assets/jcode/herdr-agent-state.sh` - the installed reporter, `/bin/sh` + inline `python3`. One script for all five events; it reads `JCODE_HOOK_EVENT` because jcode passes hook metadata in env vars and writes nothing to stdin.
  - Event to state: `session_start` = idle, `turn_start` and `post_tool` = working, `turn_end` = idle when `JCODE_HOOK_STATUS=ok` else blocked, `session_end` = `pane.release_agent`.
  - `turn_start` fires before the model streams, so a turn that only thinks still reports working. That is why jcode is a lifecycle authority and not a session-only integration.
  - Guards, all exit 0 silently: `HERDR_ENV=1`, `HERDR_SOCKET_PATH`, `HERDR_PANE_ID`, `python3` on PATH, event in the known list.
  - Swarm guard: a visible swarm worker pane runs this hook too and already reports for itself. The script reads `$JCODE_HOME/sessions/<id>.json` and skips when `parent_id` is set.
  - `seq = time.time_ns()`. herdr keeps the last seq per `(pane, source)` and drops anything not greater, so a per-session counter would be ignored for the life of the pane.
  - On `session_start` it sends `pane.report_agent_session` first (seq N) and then `pane.report_agent` (seq N+1). Without the anchor, `route_full_lifecycle_hook_report` answers `ok` and discards the state.
- `src/integration/mod.rs` - `JCODE_HOOK_ASSET`, `JCODE_HOOK_INSTALL_NAME`, `JCODE_HOOK_EVENTS`, `JCODE_INTEGRATION_VERSION = 1`. Unix only, no PowerShell asset.
- `src/integration/env.rs` - `jcode_dir()`: `$JCODE_HOME` else `~/.jcode`.
- `src/integration/targets.rs` - `install_jcode` / `uninstall_jcode`: write `~/.jcode/hooks/herdr-agent-state.sh`, chmod +x, then edit `~/.jcode/config.toml`.
- `src/integration/config_edit.rs` - `build_jcode_config_with_hooks` / `remove_jcode_config_hooks`. jcode keeps hooks in `[hooks]` and accepts one command or an array per event, so install *appends* and uninstall removes only our command.
- `src/integration/registry.rs`, `actions.rs`, `types.rs`, `src/api/schema/integrations.rs` - the `IntegrationTarget::Jcode` target and its arity constants.
- `src/cli/integration.rs` - since upstream `fff6c820` the CLI splits targets into `IntegrationCommandTarget::{Builtin, Letta}`; jcode is `Builtin(IntegrationTarget::Jcode)`.
- `src/detect/mod.rs` - `Agent::Jcode` in `ALL` and in `full_lifecycle_hook_authority`, deliberately **not** in `SCREEN_MANIFEST_AGENTS`: two authorities on one pane is the bug this avoids.
- `src/agent_resume.rs` + `src/persist/restore.rs` - `herdr:jcode` is an official source, so a snapshot restores as `jcode --resume <id>`. An unofficial source reporting the same agent is refused.
- `src/terminal/state.rs` - `("herdr:jcode", "jcode", Some("resume" | "new"))` allowed as a session *replacement*. `jcode --resume <id>` fires `session_start` twice; without replacement the pane stays anchored to the throwaway id and every later report is dropped.

**Verify.** `cargo nextest run fork_contract` covers install, idempotent reinstall, uninstall preserving a foreign hook command, TOML round-trip in both string forms, the hook-authority-without-manifest gate, the resume re-anchor, the resume restore plan, and the actual JSON-RPC the script puts on a stand-in socket (that last one shells out to `scripts/verify_jcode_hook.py`, and skips cleanly when `python3` is missing).

**Replication hazards.**

- Upstream adds integration targets constantly, and in the 2026-09-27 port its new `Letta` landed on the exact lines our `Jcode` occupies: `IntegrationTarget::ALL`, `integration_specs()`, `Agent::ALL`, `SCREEN_MANIFEST_AGENTS` array sizes, the `use` lists in `integration/{actions,targets}.rs`, and three translated `integrations.mdx` tables. Take upstream's list and re-add `Jcode`; then fix the array-size annotations, which the compiler catches and a conflict resolution does not.
- Keep `Jcode` out of `SCREEN_MANIFEST_AGENTS`. If upstream ever ships a bundled `jcode.toml` manifest, drop ours or drop the hook authority, never both.
- `HERDR_INTEGRATION_VERSION=1` in the asset must match `JCODE_INTEGRATION_VERSION`.

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

**Still dead on purpose.** `src/ui/status.rs::idle_age_at` and `AgentPanelEntry.aged_from` keep `#[allow(dead_code)]`: they are the `AppState`-side equivalents, unused because the client classifies from the wire. Delete them or wire them, but do not assume their presence means the glyph path works - check `render_agent_row` instead.

---

## F5 - agents above spaces, subagent nesting

**Purpose.** The agents list is what gets looked at; it belongs on top. A coordinator's subagents belong under the coordinator, not scattered.

**How it works.**

- `src/ui/sidebar.rs` - `sidebar_section_heights` returns `(top, bottom)`, `split_ratio` means the *agents'* share, section rects return `(spaces, agents)`. The agents header is two rows, the divider moved into the spaces header.
- Footer collision, load-bearing: the sidebar's collapse-toggle glyph always renders at the sidebar's last row, so with spaces at the bottom the `new`/`menu` footer collided with it. `footer_y` is clamped with `.min(area.bottom() - 2)` in both `src/client/shell/sidebar.rs` and `endpoint_sidebar.rs`.
- `src/ui.rs` - `SUBAGENT_PARENT_TOKEN = "parent_pane"`, a plain metadata token so nesting needs no wire, protocol or persistence change and a dead parent degrades to a flat row. Reported by the spawning agent: `herdr pane report-metadata <child> --source <id> --token parent_pane=<parent public pane id>`. herdr overwrites `HERDR_PANE_ID` in every pane it launches, so only the splitting parent can report the link.
- `src/client/shell/agent_sidebar.rs` - `nested_agent_pane_ids` returns `(pane_id, is_child)` and `ordered_agent_pane_ids` delegates to it, so hit map, keyboard navigation and drawn rows cannot disagree. Children keep incoming order, descendants flatten to one indent level, a cycle or missing parent stays flat, child rows draw `├─`/`└─`. Upstream's separate `agent_row()` helper (federated sidebar) passes `nested: false`.
- `src/ui/sidebar.rs::nest_subagent_entries` - the same ordering on the server-side `AgentPanelEntry` list.

**Verify.** `fork_contract_subagent_panes_render_indented_under_their_parent` asserts the drawn rows, `fork_contract_*` tests for the agents-header-above-spaces-header order and for the footer clamp assert the composed frame, not the arithmetic.

**Replication hazard, the big one.** This feature was written twice. v0.9.0 moved agent-row *rendering* into the client shell and kept only the half that computes the hierarchy, so panes rendered flat with nothing failing. After any upstream merge, check both halves: the entry data **and** the row rendering. The contract test now covers exactly that, which is why it must never move into an upstream-owned file.

---

## Fork-owned deviation: the frozen endpoint-shape fixture

`tests/fixtures/endpoint-method-shapes-v1.json` is a compatibility contract, and upstream's rule is never to re-bless a generation-1 expectation. The fork's `IntegrationTarget::Jcode` variant changes the `integration.install` shape digest, so the test failed permanently. In this fork our binary is both client and server, so the fork re-blesses its own copy, with the reasoning at the assertion site in `src/server/client_commands.rs`.

Two consequences a future port must honour: never carry this re-bless upstream, and re-generate it again after re-adding `Jcode` on the next merge. It also means a fork-built client talking to an upstream-built server is out of contract on that method - we do not do that, and `IntegrationTarget` has no `Unknown` fallback to make it safe if we ever did.

---

## Fork-local docs and rules

- `docs/next/known-issues/stale-herdr-pane-id-agent-status.md` - a pane's public id changes but the agent's baked-in `HERDR_PANE_ID` does not, so it reports to a pane it no longer occupies. Diagnosed, fix deferred.
- `docs/next/known-issues/pane-silently-rejects-agent-reports.md` - untracked in git; a suppression latch rejecting every report on one pane whose address is correct. Read it before confusing the two.
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

---

## Replication procedure onto a newer upstream

1. `git fetch upstream && git checkout -b port/<date> master`.
2. `git merge upstream/master`. Expect the conflicts to cluster exactly where upstream adds its own integrations and agents: `src/integration/*`, `src/detect/mod.rs`, `src/cli/integration.rs`, `src/agent_resume.rs`, `src/client/shell/agent_sidebar.rs`, and the translated `integrations.mdx` tables.
3. Resolve by taking upstream's structure and re-adding our entry. Never keep our copy of a file upstream moved or rewrote. Then fix the array-size annotations the compiler flags.
4. `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
5. **`cargo nextest run fork_contract`.** This is the whole point of the file: a green run means every feature above survived, and a red one names which did not. Do not go exploring the features by hand first.
6. Full `cargo nextest run`, compared against the numbers in Ground truth. The frozen endpoint-shape fixture needs re-blessing again (see above).
7. Update Ground truth and the commit map here, in the same commit.
8. A fork feature you had to re-implement differently gets its section rewritten here and its contract test strengthened in the same commit - a test that could not catch the loss you just repaired is the next port's silent failure.
