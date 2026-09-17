#!/bin/sh
# installed by herdr
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=jcode
# HERDR_INTEGRATION_VERSION=1

set -u

# jcode passes hook metadata in JCODE_HOOK_* environment variables and writes
# nothing to stdin, so the event name comes from JCODE_HOOK_EVENT rather than
# an argument or a parsed payload.
event="${JCODE_HOOK_EVENT:-}"
case "$event" in
  session_start|turn_start|turn_end|post_tool|session_end) ;;
  *) exit 0 ;;
esac

[ "${HERDR_ENV:-}" = "1" ] || exit 0
[ -n "${HERDR_SOCKET_PATH:-}" ] || exit 0
[ -n "${HERDR_PANE_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

python3 - <<'PY'
import json
import os
import random
import socket
import time

source = "herdr:jcode"
agent = "jcode"
event = os.environ.get("JCODE_HOOK_EVENT", "")
pane_id = os.environ.get("HERDR_PANE_ID")
socket_path = os.environ.get("HERDR_SOCKET_PATH")
session_id = os.environ.get("JCODE_HOOK_SESSION_ID") or None

if not pane_id or not socket_path:
    raise SystemExit(0)


def has_parent_session(session_id):
    """True when this session is a swarm worker spawned into its own pane.

    jcode's swarm can spawn a worker as a visible pane of its own, and that
    pane runs this hook too. Such a session records the coordinator in
    `parent_id`; headless and inline workers, like ordinary sessions, leave it
    null. Reporting for a visible worker would attach a second lifecycle
    authority to a pane that already has one, so skip it.
    """
    if not session_id:
        return False
    home = os.environ.get("JCODE_HOME") or os.path.join(
        os.path.expanduser("~"), ".jcode"
    )
    path = os.path.join(home, "sessions", "%s.json" % session_id)
    try:
        with open(path, encoding="utf-8", errors="ignore") as handle:
            return bool(json.load(handle).get("parent_id"))
    except Exception:
        return False


if has_parent_session(session_id):
    raise SystemExit(0)

# jcode fires turn_start before the model streams, so a turn that only thinks
# still reports working. turn_end carries STATUS, which is the only signal
# that the turn needs a human rather than having finished cleanly.
if event == "session_start":
    state = "idle"
elif event in ("turn_start", "post_tool"):
    state = "working"
elif event == "turn_end":
    state = "idle" if os.environ.get("JCODE_HOOK_STATUS", "ok") == "ok" else "blocked"
else:
    state = None

request_id = f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}"
# Monotonic across sessions on purpose: herdr keeps the last seq per
# (pane, source) and drops anything not greater, so a counter that restarts
# with each session is silently ignored for the life of the pane.
report_seq = time.time_ns()
session_start_source = (
    "resume" if os.environ.get("JCODE_HOOK_SOURCE") == "resume" else "new"
)

# Herdr keeps the last seq per (pane, source) and drops anything not greater.
# On session_start two requests go out, so the anchor takes this seq and the
# state report below must take a strictly higher one.
anchor_seq = report_seq
if session_id and event == "session_start":
    report_seq += 1

if state is None:
    request = {
        "id": request_id,
        "method": "pane.release_agent",
        "params": {
            "pane_id": pane_id,
            "source": source,
            "agent": agent,
            "seq": report_seq,
        },
    }
else:
    params = {
        "pane_id": pane_id,
        "source": source,
        "agent": agent,
        "state": state,
        "seq": report_seq,
    }
    if session_id:
        params["agent_session_id"] = session_id
        if event == "session_start":
            params["session_start_source"] = session_start_source
    request = {
        "id": request_id,
        "method": "pane.report_agent",
        "params": params,
    }

message = os.environ.get("JCODE_HOOK_TOOL_NAME") if event == "post_tool" else None
if message and state is not None:
    request["params"]["message"] = message

def send(request):
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.settimeout(0.5)
        client.connect(socket_path)
        client.sendall((json.dumps(request) + "\n").encode())
        try:
            client.recv(4096)
        except Exception:
            pass
        client.close()
    except Exception:
        pass


# A full lifecycle authority must anchor its session before Herdr will apply
# its state. Without this, `pane.report_agent` is answered with `ok` and then
# silently discarded, because `route_full_lifecycle_hook_report` has no
# anchored session to match the report against and no live authority yet.
# Verified 2026-08-24 against a real server: report_agent alone left the pane
# at its previous state, and the same report after report_agent_session
# applied immediately.
if session_id and event == "session_start":
    send(
        {
            "id": request_id + ":session",
            "method": "pane.report_agent_session",
            "params": {
                "pane_id": pane_id,
                "source": source,
                "agent": agent,
                "agent_session_id": session_id,
                "session_start_source": session_start_source,
                "seq": anchor_seq,
            },
        }
    )

send(request)
PY

exit 0
