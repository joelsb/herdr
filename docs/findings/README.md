# Findings

One file per diagnosed error in this fork or in the binary built from it: symptom, the wrong diagnosis it invites, mechanism, fix, proof for both the failing and the passing case, status, do-not-undo.

Status is `proven working` only with an observation naming who saw it and when. `unverified` and `partially proven` are allowed and better than a wrong `proven working`.

| Date | Finding | Status |
|---|---|---|
| 2026-09-27 | [A locally built herdr with a Debug vt lib burns 64% of a core and stalls every keystroke](2026-09-27-debug-vt-lib-burns-a-core.md) | partially proven - static check observed by Joel, runtime CPU not yet observed |
