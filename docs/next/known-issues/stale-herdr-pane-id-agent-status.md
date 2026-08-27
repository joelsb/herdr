# Stale `HERDR_PANE_ID` makes every jcode agent show as idle

**Status:** diagnosed, not fixed. Joel deferred the fix on 2026-08-27.
**Symptom:** every agent row in the herdr sidebar shows `idle`, including agents that are visibly working right now.
**Impact:** cosmetic. The agents themselves run fine; only their status indicator is wrong.

Read this whole file before touching anything. Everything below was verified against the running system on 2026-08-27, not inferred from the code.

## The bug in one line

Each agent reports its state to herdr using a pane ID baked into its process environment at spawn. When a pane's public ID changes, that variable does not, so the agent keeps reporting to a pane it no longer occupies.

## Verified evidence

This session, on the live server:

```
$ echo $HERDR_PANE_ID
w11:p1

$ herdr pane.get w11:p1  ->  cwd .../MYNE/Work/DataInnovation/datainnovation-plugin
$ herdr pane.get w1M:p1  ->  cwd .../MYNE/Projects/tools/jcode      <- where the agent actually runs
```

The agent running in `w1M:p1` was reporting to `w11:p1`, a different workspace entirely.

Sending the report by hand to the correct pane worked immediately:

```python
# state_change_seq went 11 -> 25 and the row flipped to "working"
pane.report_agent {"pane_id": "w1M:p1", "source": "herdr:jcode",
                   "agent": "jcode", "state": "working", "seq": <time.time_ns()>}
```

So the socket, the API, the hook script and the state pipeline are all healthy. **The only thing wrong is the address.**

## Why it fails silently, with no fallback

Two design facts combine into a silent failure:

1. **`HERDR_PANE_ID` is frozen at spawn.** `src/pane.rs:146` sets it with `CommandBuilder::env` when the pane's process is created. A process's environment cannot be changed from outside afterwards, so the value cannot be corrected in place.

2. **jcode has no screen-scan fallback, on purpose.** `src/detect/mod.rs:96` excludes jcode from `SCREEN_MANIFEST_AGENTS`:

   > `Jcode is deliberately absent: its integration is a full lifecycle authority (see full_lifecycle_hook_authority), so screen manifest fallback would be a second, competing source of truth.`

   For every other agent, a missed hook is covered by reading the pane's screen. For jcode the hook *is* the only source, so a misaddressed report means the row never updates at all.

Nothing errors, because the stale ID usually still resolves to a real pane. The report lands on a stranger's row and both panes look wrong: one never goes busy, the other may flicker.

## What is NOT the cause

Ruled out during diagnosis, so nobody re-treads this:

- **Not the live handoff.** The handoff was suspected first because the symptom appeared right after one. The env var was already stale beforehand.
- **Not the `close_pane_if_idle` keybinding** added the same day (commit `04278c8`).
- **Not the handoff title loss.** That was a real, separate bug in the same area, fixed in commit `61587c3`. Fixing it did not fix this.
- **Not a broken socket, hook script, or detection pipeline.** All three were exercised directly and work.

Still unknown: **what causes the IDs to drift in the first place** - handoff, workspace recreation, or something older. Option 1 below makes the cause irrelevant, which is a reason to prefer it over chasing the trigger.

## Fix options

### Option 1 (recommended): herdr resolves the pane itself

Stop trusting `HERDR_PANE_ID` as the sole address. When a `pane.report_agent` / `pane.release_agent` call arrives, map the calling process back to its pane by walking the process tree to the owning PTY, and use the env var only as a hint.

- Self-healing: fixes the currently running agents with no restarts.
- Cannot go stale again, whatever causes the renumbering.
- Costs real code in the hook-handling path, and needs a rule for what wins when hint and resolution disagree.

Relevant code: `pane.report_agent` handling in `src/app/api/panes.rs`; pane lookup in `src/app/ids.rs` (`parse_pane_id`); existing process-tree helpers in `src/detect/mod.rs` (`foreground_job`, `foreground_process_group_id`).

### Option 2 (partial): alias old public pane IDs after renumbering

`src/app/mod.rs` (`new_from_handoff`) already builds `pane_id_aliases` for raw IDs, but **nothing populates `public_pane_id_aliases`**, which is the `w11:p1` string form the hook actually sends. `parse_pane_id` consults that map first (`src/app/ids.rs:107`), so filling it would redirect stale reports.

- Much smaller change than option 1.
- Only covers renumbering paths that build aliases. If the drift has another cause, it does not help.

### Option 3 (no code): restart the affected agents

They re-read the environment at spawn and pick up correct IDs.

- Works immediately, zero risk.
- Recurs the next time a pane ID changes.

## How to verify any fix

Do not trust a green unit test alone; this bug lives in the gap between a process's environment and the server's view.

1. With an agent running, confirm the mismatch is real:
   `echo $HERDR_PANE_ID` inside the pane, then compare `pane.get` on that ID against the pane the agent truly occupies.
2. Make the agent do work and watch `agent.list`. Its `agent_status` must reach `working` and `state_change_seq` must climb.
3. Plant the defect: force a stale ID and require the status to still resolve correctly. A test that never failed proves nothing here.
4. Check the innocent pane too. The stale target (`w11:p1` in the original report) must NOT be driven by another pane's activity.

## Useful commands

```bash
# Every agent's reported status and sequence number
herdr agent list

# What herdr thinks a pane ID refers to
herdr pane get <pane-id>

# What is actually running in a pane (process-tree truth)
herdr pane process-info <pane-id>
```

Raw API over the socket at `~/.config/herdr/herdr.sock`, newline-delimited JSON:

```
{"id":"x","method":"agent.list","params":{}}
{"id":"x","method":"pane.get","params":{"pane_id":"w1M:p1"}}
{"id":"x","method":"pane.report_agent","params":{"pane_id":"...","source":"herdr:jcode","agent":"jcode","state":"working","seq":<ns>}}
```

**`seq` must be strictly increasing per (pane, source)** or the report is dropped without error. The hook uses `time.time_ns()` for this reason - a per-session counter restarting at zero is silently ignored for the life of the pane.

## Files involved

| Path | Why it matters |
|---|---|
| `src/pane.rs:146` | Sets `HERDR_PANE_ID` at spawn. The frozen value. |
| `src/detect/mod.rs:96` | Comment explaining why jcode has no screen-scan fallback. |
| `src/app/ids.rs:107` | `parse_pane_id`, consults `public_pane_id_aliases` first. |
| `src/app/mod.rs` | `new_from_handoff`, builds `pane_id_aliases` but not the public ones. |
| `src/app/api/panes.rs` | Handles `pane.report_agent`. |
| `~/.jcode/hooks/herdr-agent-state.sh` | The jcode-side hook. Installed by herdr; **it is overwritten on reinstall, so do not patch it there.** |

## One caveat about the current live state

While diagnosing, `w1M:p1` was set to `working` by hand and then set back to `idle`. No other pane was modified. If a row looks wrong in a way this document does not explain, that is not the cause.
