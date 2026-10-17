#!/usr/bin/env python3
"""A fake coding agent speaking the Agent Client Protocol on stdio, for
zcode's tests: no network, no account.

    python3 scripts/fake_acp_agent.py

The prompt's first word picks what it does (the rest are arguments; paths
are relative to the session's folder):

    hello                    streams a thought and a Markdown answer
    read <path>              a read tool call: fs/read_text_file through the client,
                             then says the file's first line and length
    edit <path> <old> => <new>
                             reads the file through the client, asks permission
                             (with the diff), writes it back through the client
    create <path> <text>     writes a new file through the client (no permission asked)
    self-edit <path> <old> => <new>
                             changes the file on disk itself, then reports the diff
    outside                  tries to write outside the project
    plan                     sends a plan, then updates it
    slow                     streams for ~10 s; stops early on session/cancel
    echo                     answers with the prompt's content blocks as JSON
    mode                     reports the current mode
    crash                    exits with a message on stderr

Environment: FAKE_ACP_LOG=<file> appends every message it receives and
sends; FAKE_ACP_REQUIRE_AUTH=1 makes session/new fail with "authentication
required" until `authenticate` is called.
"""

import json
import os
import queue
import sys
import threading
import time

LOG = os.environ.get("FAKE_ACP_LOG")
REQUIRE_AUTH = os.environ.get("FAKE_ACP_REQUIRE_AUTH") == "1"

write_lock = threading.Lock()
pending = {}  # our request id → [event, response]
pending_lock = threading.Lock()
incoming = queue.Queue()
next_id = [100]
state = {"authenticated": False, "sessions": {}, "cancelled": set()}


def log(direction, message):
    if LOG:
        with open(LOG, "a") as f:
            f.write(json.dumps({"dir": direction, "msg": message}) + "\n")


def send(message):
    message["jsonrpc"] = "2.0"
    log("out", message)
    with write_lock:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()


def request(method, params):
    """Asks the client something and waits for the answer: (result, error)."""
    with pending_lock:
        next_id[0] += 1
        rid = next_id[0]
        slot = [threading.Event(), None]
        pending[rid] = slot
    send({"id": rid, "method": method, "params": params})
    slot[0].wait()
    response = slot[1]
    return response.get("result"), response.get("error")


def notify(method, params):
    send({"method": method, "params": params})


def update(session, update):
    notify("session/update", {"sessionId": session, "update": update})


def say(session, text, kind="agent_message_chunk"):
    update(session, {"sessionUpdate": kind, "content": {"type": "text", "text": text}})


def reader():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)
        log("in", message)
        if "method" not in message and "id" in message:
            with pending_lock:
                slot = pending.pop(message["id"], None)
            if slot:
                slot[1] = message
                slot[0].set()
            continue
        if message.get("method") == "session/cancel":
            state["cancelled"].add(message["params"]["sessionId"])
            continue
        incoming.put(message)
    os._exit(0)


def handle(message):
    method, rid, params = message.get("method"), message.get("id"), message.get("params") or {}
    if method == "initialize":
        send({"id": rid, "result": {
            "protocolVersion": 1,
            "agentCapabilities": {"loadSession": False, "promptCapabilities": {"embeddedContext": True, "image": False}},
            "authMethods": [{"id": "fake-login", "name": "Sign In (fake)", "description": "Signs in to the fake agent"}],
            "agentInfo": {"name": "fake-acp-agent", "version": "1.0"},
        }})
    elif method == "authenticate":
        state["authenticated"] = True
        send({"id": rid, "result": {}})
    elif method == "session/new":
        if REQUIRE_AUTH and not state["authenticated"]:
            send({"id": rid, "error": {"code": -32000, "message": "Authentication required"}})
            return
        session = f"fake-session-{len(state['sessions']) + 1}"
        state["sessions"][session] = {"cwd": params["cwd"], "mode": "default"}
        send({"id": rid, "result": {"sessionId": session, "modes": {"currentModeId": "default", "availableModes": [
            {"id": "default", "name": "Always Ask"}, {"id": "plan", "name": "Plan Mode"}]}}})
        update(session, {"sessionUpdate": "available_commands_update", "availableCommands": [
            {"name": "init", "description": "Set the project up"}]})
    elif method == "session/set_mode":
        state["sessions"][params["sessionId"]]["mode"] = params["modeId"]
        send({"id": rid, "result": {}})
        update(params["sessionId"], {"sessionUpdate": "current_mode_update", "currentModeId": params["modeId"]})
    elif method == "session/prompt":
        threading.Thread(target=prompt, args=(rid, params), daemon=True).start()
    elif rid is not None:
        send({"id": rid, "error": {"code": -32601, "message": f"no method {method}"}})


def prompt_text(blocks):
    return "".join(b.get("text", "") for b in blocks if b.get("type") == "text").strip()


def prompt(rid, params):
    session = params["sessionId"]
    state["cancelled"].discard(session)
    cwd = state["sessions"][session]["cwd"]
    blocks = params["prompt"]
    text = prompt_text(blocks)
    word, _, rest = text.partition(" ")
    stop = "end_turn"
    try:
        stop = run(session, cwd, word, rest.strip(), blocks) or "end_turn"
    except Exception as e:  # noqa: BLE001
        say(session, f"fake agent error: {e}")
    send({"id": rid, "result": {"stopReason": stop}})


def path_in(cwd, rel):
    return os.path.join(cwd, rel)


def run(session, cwd, word, rest, blocks):
    if word == "hello":
        say(session, "The user says hello.", "agent_thought_chunk")
        for chunk in ["# Hello", " from the fake agent\n\n", "- one\n", "- two\n"]:
            say(session, chunk)
            time.sleep(0.05)
    elif word == "read":
        path = path_in(cwd, rest)
        update(session, {"sessionUpdate": "tool_call", "toolCallId": "read-1", "title": f"Read {rest}", "kind": "read",
                         "status": "in_progress", "locations": [{"path": path, "line": 1}]})
        result, error = request("fs/read_text_file", {"sessionId": session, "path": path})
        if error:
            update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "read-1", "status": "failed",
                             "content": [{"type": "content", "content": {"type": "text", "text": error["message"]}}]})
            say(session, f"Couldn't read it: {error['message']}")
            return
        content = result["content"]
        first = content.splitlines()[0] if content else ""
        update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "read-1", "status": "completed",
                         "content": [{"type": "content", "content": {"type": "text", "text": content[:400]}}]})
        say(session, f"First line: `{first}` ({len(content)} characters)")
    elif word in ("edit", "self-edit"):
        rel, _, change = rest.partition(" ")
        old, _, new = change.partition(" => ")
        path = path_in(cwd, rel)
        if word == "edit":
            result, error = request("fs/read_text_file", {"sessionId": session, "path": path})
            if error:
                say(session, f"Couldn't read it: {error['message']}")
                return
            before = result["content"]
        else:
            with open(path) as f:
                before = f.read()
        after = before.replace(old, new, 1)
        line = before[:before.find(old)].count("\n") + 1 if old in before else 1
        # Like Claude Code's adapter, a change it makes itself is reported
        # as just the changed line.
        if word == "self-edit":
            start = before.rfind("\n", 0, before.find(old)) + 1
            end = before.find("\n", start) + 1 or len(before)
            diff = {"oldText": before[start:end], "newText": before[start:end].replace(old, new, 1)}
        else:
            diff = {"oldText": before, "newText": after}
        call = {"toolCallId": "edit-1", "title": f"Edit {rel}", "kind": "edit", "status": "pending",
                "locations": [{"path": path, "line": line}],
                "content": [dict(type="diff", path=path, **diff)]}
        update(session, dict(sessionUpdate="tool_call", **call))
        if word == "self-edit":
            with open(path, "w") as f:
                f.write(after)
            update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "edit-1", "status": "completed"})
            say(session, f"Changed {rel} myself.")
            return
        result, error = request("session/request_permission", {"sessionId": session, "toolCall": {"toolCallId": "edit-1", "title": f"Edit {rel}"},
                                 "options": [{"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                                             {"optionId": "always", "name": "Always Allow", "kind": "allow_always"},
                                             {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]})
        outcome = (result or {}).get("outcome", {})
        if outcome.get("outcome") == "cancelled":
            update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "edit-1", "status": "failed"})
            return "cancelled"
        if outcome.get("optionId") == "reject":
            update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "edit-1", "status": "failed"})
            say(session, "Not changed.")
            return
        update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "edit-1", "status": "in_progress"})
        _, error = request("fs/write_text_file", {"sessionId": session, "path": path, "content": after})
        if error:
            update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "edit-1", "status": "failed"})
            say(session, f"Couldn't write it: {error['message']}")
            return
        update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "edit-1", "status": "completed"})
        say(session, f"Changed {rel}.")
    elif word == "create":
        rel, _, text = rest.partition(" ")
        path = path_in(cwd, rel)
        update(session, {"sessionUpdate": "tool_call", "toolCallId": "create-1", "title": f"Create {rel}", "kind": "edit",
                         "status": "in_progress", "locations": [{"path": path}],
                         "content": [{"type": "diff", "path": path, "oldText": None, "newText": text + "\n"}]})
        _, error = request("fs/write_text_file", {"sessionId": session, "path": path, "content": text + "\n"})
        status = "failed" if error else "completed"
        update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "create-1", "status": status})
        say(session, f"Couldn't create it: {error['message']}" if error else f"Created {rel}.")
    elif word == "outside":
        path = os.path.join(os.path.dirname(cwd), "zcode-agent-outside.txt")
        _, error = request("fs/write_text_file", {"sessionId": session, "path": path, "content": "no\n"})
        say(session, f"refused: {error['message']}" if error else "written")
    elif word == "plan":
        entries = [{"content": "Read the code", "priority": "high", "status": "completed"},
                   {"content": "Change it", "priority": "medium", "status": "in_progress"},
                   {"content": "Test it", "priority": "low", "status": "pending"}]
        update(session, {"sessionUpdate": "plan", "entries": entries})
        say(session, "Planned.")
    elif word == "slow":
        for i in range(100):
            if session in state["cancelled"]:
                return "cancelled"
            say(session, f"chunk {i} ")
            time.sleep(0.1)
    elif word == "echo":
        shown = []
        for b in blocks:
            if b["type"] == "resource":
                shown.append({"type": "resource", "uri": b["resource"]["uri"], "text": b["resource"].get("text")})
            elif b["type"] == "resource_link":
                shown.append({"type": "resource_link", "uri": b["uri"], "name": b["name"]})
            else:
                shown.append({"type": b["type"], "text": b.get("text")})
        say(session, "```json\n" + json.dumps(shown) + "\n```")
    elif word == "mode":
        say(session, f"mode: {state['sessions'][session]['mode']}")
    elif word == "crash":
        sys.stderr.write("fake agent: something went wrong\n")
        sys.stderr.flush()
        os._exit(3)
    else:
        say(session, f"I don't know how to {word!r}.")


def main():
    threading.Thread(target=reader, daemon=True).start()
    while True:
        handle(incoming.get())


if __name__ == "__main__":
    main()
