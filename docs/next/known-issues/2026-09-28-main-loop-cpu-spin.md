# The server main loop spins: ~50% of a core with idle panes

**Status: fixed on branch `fix/jsb17-main-loop`, accept moved off the main loop.** Tracked as JSB-17.
https://linear.app/joelsb/issue/JSB-17/herdr-server-burns-50percent-of-a-core-main-loop-accept-spin-plus-idle

Read this before touching `src/server/headless/`, `src/server/client_accept.rs` or the idle-aging code in `src/app/runtime.rs`, and before believing any CPU measurement of this binary. See `FORK.md` section F7 for the fix itself; this file keeps the original diagnosis evidence and the corrected baseline below.

## Resolution (2026-09-28, JSB-17)

The original diagnosis below is real but its baseline was measured on a lightly loaded box; a fresh capture on 2026-09-28 on Joel's real Linux server (`ssh-joel`, 7 agent panes, load 6-8 on 8 cores from other agent processes) found **stock upstream herdr 0.9.1 also burns CPU under that load** - 37 CPU-seconds in 60 seconds, not the 1.7%-of-a-core reference number below, which was measured under lighter load than this box now carries. So this was never fork-only, and the "healthy = 1.7% of a core" comparison in the Symptom section understates what stock upstream does under real contention.

`strace -qq -c` on the stock build confirmed the mechanism named below: `accept4` called on every one of ~74 main-loop passes/second, all returning `EWOULDBLOCK`, each syscall costing 300-800us specifically because the box was CPU-saturated (uncontended, the same syscall costs low single-digit microseconds - see F7 for a from-quiet-Mac measurement showing no CPU delta at all). A throwaway-session capture with `HERDR_RENDER_PROF=1` (`.agents/skills/herdr-throwaway-repro/SKILL.md`) found the ~74 passes/second is not driven by PTY output volume directly: six panes each writing a few bytes every 200ms (an agent spinner's shape) is enough, because every render frame delivered to an attached client fires `ServerEvent::ClientWriterDrained` as a second, work-free wake in addition to the render itself, and the old code called `accept()` on every one of those wakes regardless.

The idle-age suspect (`repaint_due_idle_age`/`sync_idle_age_deadline`, fork feature F4) named below is **ruled out, not fixed** - see F7 for the measurement that ruled it out. Fixed by moving accept to a dedicated thread that blocks on listener readiness (`spawn_unix_client_accept_thread`, `src/server/client_accept.rs`), mirroring the accept thread Windows already had. Full mechanism, before/after numbers, and the regression test: `FORK.md` F7.

## Symptom

`herdr server` burns roughly half a core continuously, whether or not panes are producing output. Joel feels it as typed characters appearing seconds late, panes that stop scrolling, and session switches that are not instant.

Measured 2026-09-28 on the Mac: **793 minutes of CPU in 26h45 of uptime** on `~/.local/bin/herdr`, panes idle or lightly used for most of that window.

Reference for what healthy looks like: the same software on the Linux box `ssh-joel` (`herdr 0.9.1`, stock upstream, no fork patches) used `3h57m` of CPU in `9d 18h` **with live panes** - 1.7% of a core. The gap is ~30x.

## This is the second cause, not the first

On 2026-09-27 a different bug was found and fixed: the installed binary had been built with the vendored vt lib at `-Doptimize=Debug`, so `Screen.clearCells` called `Page.verifyIntegrity` on every erase-line. See `docs/findings/2026-09-27-debug-vt-lib-burns-a-core.md`.

That fix is real and still holds - VT parsing does not appear in any capture below. But it only took the burn from 64% of a core to 50%, which is how this one surfaced. **Do not re-diagnose the vt lib.** Confirm it is clean in one command before looking anywhere else:

```bash
otool -tvV ~/.local/bin/herdr \
  | awk '/^_?terminal\.Screen\.clearCells/{f=1} f&&/^[a-zA-Z_]/&&!/clearCells/{f=0} f&&/bl\t/{print $NF}' \
  | sort -u | grep -ci integrity
```

`0` means the old bug is absent and you are looking at this one.

## Where it burns

Six bursts captured in ~6 minutes at 30%, 65%, 100%, 100%, 101%, 100% of a core. Every one lands on the **main thread**, `2539/3964` samples busy, split across two call paths:

```
herdr main loop
   |
   +- accept_pending_client_connections --> __accept --> cerror/EWOULDBLOCK    1414/3964 samples
   |     non-blocking accept, called every pass, nothing to accept
   |
   +- handle_scheduled_tasks_headless --> repaint_due_idle_age                  725/3964 samples
         +- sync_idle_age_deadline
```

**1. The accept spin.** The listener is non-blocking and `accept()` is called unconditionally on each pass of the main loop; `1362` of those `1414` samples sit inside `__accept` and its error path, returning `EWOULDBLOCK`. Nothing waits for readiness, so the loop free-runs.

**2. The idle-age recompute.** `repaint_due_idle_age` and `sync_idle_age_deadline` run hot inside `handle_scheduled_tasks_headless`. This is **fork code** - feature F4 (idle aging, stale and parked buckets), see `FORK.md` section `## F4`. Upstream does not have it, so no upstream release will fix it and no port will carry the fix for you.

## Ruled out, with the evidence that ruled them out

| Suspect | Why it is not the cause |
|---|---|
| subscription streaming | `stream_subscriptions` threads sleep `3926/3955` samples - they poll politely |
| pane VT parsing | absent from all 6 captures |
| pi agent processes | 1.1 GB total, 1-20% CPU each, normal |
| memory pressure | real (945 MB swap, 71 MB free at one point) but it cannot make a main thread busy - separate problem |

## Evidence

`~/Documents/herdr-watch-20260928/` - `log.txt` plus 6 `sample-*.txt` and the matching `panes-*.json`. Copied out of `/tmp` on 2026-09-28 so a reboot cannot take them.

The watcher that produced them polls herdr's CPU every 10s and samples for 5s whenever it exceeds 25% of a core. It stops itself after 6 captures.

## How to reproduce

```bash
sample $(ps ax -o pid=,command= | awk '/local\/bin\/herdr server/ && !/awk/ {print $1; exit}') 5 -mayDie -f /tmp/x.txt
```

Two traps, both hit for real this week:

- **`pgrep -f` does not match this herdr server on this Mac.** It returns nothing while `ps ax` finds the process. Use the `ps`+`awk` form above.
- **`ps` TIME carries fractional seconds**, so bash `$(( ))` errors on a delta. Compute it in `awk`.

## Where to start

1. Copy the evidence somewhere permanent if it is not already.
2. `accept_pending_client_connections` in `src/server/client_accept.rs`, and the loop that calls it in `src/server/headless/`. The question: can accept block with a timeout, or be gated on listener readiness, instead of being called every pass?
3. `repaint_due_idle_age` and `sync_idle_age_deadline` in `src/app/runtime.rs`. The question to answer *before* editing: is the deadline recomputed on every scheduled-task pass when it could be computed once and slept on until it fires?
4. Reproduce first, fix second. A stack trace taken after a change proves nothing on its own.

## Acceptance

CPU time over a real working session, not a stack trace and not a screenshot:

```bash
ps -o etime=,time= -p <herdr pid>
```

Target is the Linux box's 1.7% of a core. "Looks better" is not the bar; 793 min / 26h45 is the number to beat.

## Do not

- **Do not verify with a single `sample` of an idle server.** It prints clean regardless of the build. That mistake was made twice this week - once against a completely unrelated PID, an orphan dev server with no panes, which produced a confident and entirely wrong "fixed".
- Do not treat `subagents_list`-style summary output, or any tool that reads a config rather than the live path, as proof. Assert against what actually runs.
