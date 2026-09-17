"""Verify the jcode reporter asset by capturing what it puts on the wire.

Usage: python3 scripts/verify_jcode_hook.py src/integration/assets/jcode/herdr-agent-state.sh

The Rust tests cover install and config editing; this covers the script those
tests install, which is otherwise only exercised by running a real agent.

Runs the real script against a stand-in Herdr socket, so what is asserted is
the actual JSON-RPC request, not a reimplementation of it.
"""
import json, os, socket, subprocess, tempfile, threading, pathlib, sys

HOOK = sys.argv[1]
tmp = tempfile.mkdtemp()
sock_path = os.path.join(tmp, "herdr.sock")
jcode_home = os.path.join(tmp, "jcodehome")
os.makedirs(os.path.join(jcode_home, "sessions"))

srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(sock_path); srv.listen(8); srv.settimeout(5)
received = []

def serve():
    while True:
        try:
            conn, _ = srv.accept()
        except Exception:
            return
        data = conn.recv(65536)
        if data:
            received.append(json.loads(data.decode().strip()))
        conn.close()

threading.Thread(target=serve, daemon=True).start()

def run(event, session_id="sess-1", extra=None):
    env = dict(os.environ)
    env.update({
        "HERDR_ENV": "1", "HERDR_PANE_ID": "w9:p1",
        "HERDR_SOCKET_PATH": sock_path, "JCODE_HOME": jcode_home,
        "JCODE_HOOK_EVENT": event, "JCODE_HOOK_SESSION_ID": session_id,
    })
    env.update(extra or {})
    before = len(received)
    subprocess.run(["bash", HOOK], env=env, timeout=10)
    return received[before:] 

fails = []
def check(name, cond, detail=""):
    print(("PASS " if cond else "FAIL ") + name + ("" if cond else "  <- " + str(detail)))
    if not cond: fails.append(name)

got = run("session_start")
# A full lifecycle authority must anchor its session first, or Herdr answers
# `ok` and silently discards the state. Regression guard for 2026-08-24.
check("session_start anchors the session FIRST",
      len(got) == 2 and got[0]["method"] == "pane.report_agent_session", got)
check("anchor carries session id and start source",
      len(got) == 2 and got[0]["params"].get("agent_session_id") == "sess-1"
      and got[0]["params"].get("session_start_source") == "new", got)
got = got[1:]
check("session_start reports idle", got and got[0]["params"]["state"] == "idle", got)
check("session_start carries session identity",
      got and got[0]["params"].get("agent_session_id") == "sess-1", got)
check("source is herdr:jcode", got and got[0]["params"]["source"] == "herdr:jcode", got)
check("agent is jcode", got and got[0]["params"]["agent"] == "jcode", got)
check("method is pane.report_agent", got and got[0]["method"] == "pane.report_agent", got)

got = run("turn_start")
check("turn_start reports working", got and got[0]["params"]["state"] == "working", got)

got = run("turn_end", extra={"JCODE_HOOK_STATUS": "ok"})
check("turn_end ok reports idle", got and got[0]["params"]["state"] == "idle", got)

got = run("turn_end", extra={"JCODE_HOOK_STATUS": "error"})
check("turn_end error reports blocked", got and got[0]["params"]["state"] == "blocked", got)

got = run("post_tool", extra={"JCODE_HOOK_TOOL_NAME": "Bash"})
check("post_tool reports working", got and got[0]["params"]["state"] == "working", got)
check("post_tool names the tool", got and got[0]["params"].get("message") == "Bash", got)

got = run("session_end")
check("session_end releases", got and got[0]["method"] == "pane.release_agent", got)

# Sequence numbers must strictly increase, or Herdr drops the later report.
seqs = [r["params"]["seq"] for r in received]
check("seq strictly increases", all(b > a for a, b in zip(seqs, seqs[1:])), seqs)

# A swarm worker with its own visible pane must report nothing.
pathlib.Path(jcode_home, "sessions", "worker.json").write_text(
    json.dumps({"parent_id": "coord-1", "status": "Active"}))
got = run("turn_start", session_id="worker")
check("visible swarm worker is skipped", got == [], got)

# A normal session whose file has no parent_id must still report.
pathlib.Path(jcode_home, "sessions", "plain.json").write_text(
    json.dumps({"parent_id": None, "status": "Active"}))
got = run("turn_start", session_id="plain")
check("headless/normal session still reports", len(got) == 1, got)

# Outside Herdr the hook must be inert.
env = dict(os.environ); env.pop("HERDR_ENV", None)
env.update({"JCODE_HOOK_EVENT": "turn_start", "HERDR_PANE_ID": "w9:p1",
            "HERDR_SOCKET_PATH": sock_path})
before = len(received)
r = subprocess.run(["bash", HOOK], env=env, timeout=10)
check("no-op outside herdr (exit 0, silent)", r.returncode == 0 and len(received) == before)

# A dead socket must not fail the turn.
env = dict(os.environ)
env.update({"HERDR_ENV": "1", "HERDR_PANE_ID": "w9:p1",
            "HERDR_SOCKET_PATH": os.path.join(tmp, "gone.sock"),
            "JCODE_HOOK_EVENT": "turn_start", "JCODE_HOOK_SESSION_ID": "x"})
r = subprocess.run(["bash", HOOK], env=env, timeout=10)
check("unreachable socket still exits 0", r.returncode == 0, r.returncode)

print("\n%d checks, %d failed" % (16, len(fails)))
sys.exit(1 if fails else 0)
