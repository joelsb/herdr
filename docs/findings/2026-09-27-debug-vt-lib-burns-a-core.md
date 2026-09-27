# A locally built herdr with a Debug vt lib burns 64% of a core and stalls every keystroke

## Symptom

The Mac felt slow in a way that pointed at everything except herdr: switching sessions was not instant, typed characters appeared in the terminal seconds late, and a scrolling pane would freeze until it caught up.

`herdr server` (the installed `~/.local/bin/herdr`, built Sep 10, `herdr 0.9.0`) had accumulated **81 minutes of CPU in 126 minutes of uptime** - 64% of a core, sustained, with 5 idle-to-normal pi panes open.

```
joelsbastos 3140 50.2 ... /Users/joelsbastos/.local/bin/herdr server
```

No error string. Nothing logged. The only signal is CPU time.

## The wrong diagnosis it invites

Everything visible points away from the real cause:

- **"Too many agents."** 7 `pi` processes were running. Together they used 1.1 GB and 1-20% CPU each - normal, and not the hog.
- **"Memory pressure."** 15 GB used of 16 GB with 5.1 GB compressed looks alarming and matches the symptom exactly. But `vm.swapusage` showed `0 swapins, 0 swapouts`: the machine was tight, not thrashing. Tempting and wrong.
- **"A terminal multiplexer cannot be the problem, it just forwards bytes."** It does not. It runs a full VT parser over every byte an agent prints and mutates a screen model, so it is exactly where a per-cell cost explodes.

## Mechanism

`vendor/libghostty-vt/src/terminal/Screen.zig:1568` `clearCells` ends with:

```zig
defer {
    page.pauseIntegrityChecks(false);
    page.assertIntegrity();
    self.assertIntegrity();
}
```

`Page.assertIntegrity` (`vendor/libghostty-vt/src/terminal/page.zig:364`) is `if (comptime build_options.slow_runtime_safety)`. When that is false the call compiles to nothing. When it is true it calls `verifyIntegrity`, which does **not** check the row that changed: it walks the entire page, every cell, and builds two `AutoHashMap`s (`styles_seen`, `hyperlinks_seen`) in a fresh arena, then frees them.

`slow_runtime_safety` is set in `vendor/libghostty-vt/src/build/Config.zig:583` and is true for exactly one value: `-Doptimize=Debug`.

The amplifier is what a TUI agent does to a terminal. pi repaints several times a second and each repaint emits one `erase line` (CSI K) per row. 5 panes x ~63 rows x several repaints per second means thousands of `clearCells` per second, each one paying a full-page walk plus two hashmap builds instead of a memset over one row.

The reason it is felt as **input lag** rather than a hot fan is the lock. That work runs holding the pane's terminal mutex, and herdr's main thread needs the same mutex to answer every API request - a keystroke, a scroll, a view sync - through `sync_foreground_client_state`, then `compute_pane_infos_for_tab`, then `PaneTerminal::alternate_screen_active`. In a 3-second sample the main thread spent **2055 of 2487 samples** parked in `_pthread_mutex_firstfit_lock_wait`. Every keypress queued behind an integrity check of a screen nobody asked to validate.

How the Sep 10 build got a Debug vt lib is **unverified**. `build.rs:54` defaults to `ReleaseFast` regardless of the cargo profile, and there is no `LIBGHOSTTY_VT_OPTIMIZE` in any shell rc on this machine, so the likeliest explanation is that it was set in that one build's environment. The stale `vendor/libghostty-vt/zig-out/lib/libghostty-vt.a` (dated Aug 25) was suspected of poisoning later builds and was cleared of that suspicion - see proof below.

## Fix

Rebuild from `master` with the two overrides in `FORK.md` (`## Build environment on this Mac`) and install over `~/.local/bin/herdr`.

| | old | new |
|---|---|---|
| file | `Sep 10 23:30`, 23,878,656 bytes | `Sep 27 12:55`, 22,234,816 bytes |
| sha256 (first 16) | `19e630f2dcafc64e` | `368649ed9fb49aaf` |

No source change was needed. The bug was entirely in how the binary was built.

## Proof

The check that decides it is **static**, run on a binary sitting on disk - disassemble `clearCells` and look for a call into the integrity path:

```bash
otool -tvV ~/.local/bin/herdr \
  | awk '/^_?terminal\.Screen\.clearCells/{f=1} f&&/^[a-zA-Z_]/&&!/clearCells/{f=0} f&&/bl\t/{print $NF}' \
  | sort -u | grep -ci integrity
```

- failing case, `~/.local/bin/herdr` before the swap: **2** - `_terminal.page.Page.verifyIntegrity`, `_terminal.Screen.assertIntegrity`
- passing case, `~/.local/bin/herdr` after the swap: **0**
- passing case, `~/.treehouse/herdr-f0f500/1/herdr/target/debug/herdr` built from current `master`: **0**

That last line is what cleared the stale `zig-out` suspicion: a cargo **debug** build still comes out clean, because `build.rs` builds the zig vt lib at `ReleaseFast` independently of the cargo profile.

Runtime evidence of the failing case, from `sample 3140 3 -mayDie`:

- `53` frames in `terminal.page.Page.verifyIntegrity`
- main thread: `2055/2487` samples in `_pthread_mutex_firstfit_lock_wait`
- hot leaf chain: `poll_pty_and_wake`, `read_once`, `process_pty_bytes`, `ghostty_terminal_vt_write`, `csiDispatchFinal`, `eraseLine`, `clearCells`, `verifyIntegrity`, `hash_map.getOrPut`, `wyhash`

### A check that returned a clean, plausible, wrong answer

The first verifier used was the sample itself:

```bash
sample $(pgrep -f 'herdr server' | head -1) 3 -mayDie -f /tmp/h.txt && grep -c verifyIntegrity /tmp/h.txt
```

It printed `0` and the fix was declared proven. **Both halves were wrong.** `pgrep ... | head -1` picked up an orphan `target/debug/herdr` dev server left over from a treehouse lease, not the installed binary - and that server had no panes producing output, so an idle server prints `0` whatever it was built from. A sample-based check only means something while panes are actively streaming, and only against the PID you actually care about.

Correct method: the `otool` disassembly above. It needs no running server, no load, and no PID.

## Status

**Partially proven.** The binary swap and the static verifier were observed by Joel and this session on 2026-09-27: `2` before, `0` after. The runtime CPU improvement has **not** been observed yet, because at the time of writing no server is running the new binary - the four processes alive are orphan `target/debug/herdr` dev servers from treehouse leases.

The real-world confirmation, once panes are up and an agent is streaming:

```bash
ps -o etime=,time= -p $(pgrep -f '.local/bin/herdr server')
```

Old build: 81 minutes of CPU per 126 minutes of uptime. A healthy build should be a couple of minutes at most.

## Do not undo

- Never install a herdr binary built with `LIBGHOSTTY_VT_OPTIMIZE=Debug`. It costs 30-60x the CPU and the only visible symptom is a machine that feels slow.
- Never verify a build of this kind with a sample of a running server. Use the `otool` check; a sample answers `0` for an idle or wrong process.
- When several herdr servers are alive, `pgrep -f 'herdr server' | head -1` is not your server. Match on the binary path.
