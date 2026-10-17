#!/usr/bin/env python3
"""A small Model Context Protocol client on stdio, for zcode's tests:
starts a server (`zcode mcp` by default), sends requests, collects
notifications. A library for scripts/e2e.py and scripts/fake_acp_agent.py,
and a command line tool:

    python3 scripts/mcp_client.py [--server <command…> --] tools
    python3 scripts/mcp_client.py call <tool> ['<json arguments>']
    python3 scripts/mcp_client.py --modern call build '{}'

The handshake revision (2025-11-25, `initialize`) is the default;
`--modern` speaks 2026-07-28 (the revision in every request's `_meta`,
`server/discover` first). Progress notifications go to stderr.
"""

import json
import os
import queue
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BINARY = os.path.join(ROOT, "target", "release", "zcode")
MODERN = "2026-07-28"
LEGACY = "2025-11-25"


class McpError(Exception):
    def __init__(self, error):
        super().__init__(f"{error.get('code')}: {error.get('message')}")
        self.code = error.get("code")
        self.data = error.get("data")


class McpClient:
    def __init__(self, command=None, env=None, cwd=None, modern=False):
        self.proc = subprocess.Popen(command or [BINARY, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, env=env, cwd=cwd, text=True, bufsize=1)
        self.modern = modern
        self.next_id = 0
        self.lock = threading.Lock()
        self.responses = {}
        self.arrived = threading.Condition(self.lock)
        self.notifications = queue.Queue()
        self.all_notifications = []
        self.stderr = []
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=lambda: self.stderr.extend(self.proc.stderr), daemon=True).start()

    def _read(self):
        for line in self.proc.stdout:
            line = line.strip()
            if not line:
                continue
            message = json.loads(line)
            if "id" in message and "method" not in message:
                with self.lock:
                    self.responses[message["id"]] = message
                    self.arrived.notify_all()
            else:
                self.all_notifications.append(message)
                self.notifications.put(message)
        with self.lock:
            self.closed = True
            self.arrived.notify_all()

    def send(self, message):
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def start(self, method, params=None, progress=None):
        """Sends a request; its id (wait for the answer with `result`)."""
        with self.lock:
            self.next_id += 1
            rid = self.next_id
        params = dict(params or {})
        meta = dict(params.get("_meta") or {})
        if self.modern:
            meta["io.modelcontextprotocol/protocolVersion"] = MODERN
            meta["io.modelcontextprotocol/clientCapabilities"] = {}
            meta["io.modelcontextprotocol/clientInfo"] = {"name": "zcode-tests", "version": "1"}
        if progress is not None:
            meta["progressToken"] = progress
        if meta:
            params["_meta"] = meta
        self.send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        return rid

    def response(self, rid, timeout=60):
        """The raw response to request `rid`, or None if none comes in time."""
        deadline = time.time() + timeout
        with self.lock:
            while rid not in self.responses:
                left = deadline - time.time()
                if left <= 0 or getattr(self, "closed", False):
                    return None
                self.arrived.wait(left)
            return self.responses.pop(rid)

    def result(self, rid, timeout=60):
        message = self.response(rid, timeout)
        if message is None:
            raise TimeoutError(f"no response to request {rid}; stderr: {''.join(self.stderr)[-400:]}")
        if "error" in message:
            raise McpError(message["error"])
        return message["result"]

    def request(self, method, params=None, timeout=60, progress=None):
        return self.result(self.start(method, params, progress), timeout)

    def initialize(self, version=LEGACY):
        result = self.request("initialize", {
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": "zcode-tests", "version": "1"},
        })
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return result

    def tools(self):
        return self.request("tools/list")["tools"]

    def call(self, name, arguments=None, timeout=600, progress=None):
        """A tool's result (`structuredContent`, `content`, `isError`)."""
        return self.request("tools/call", {"name": name, "arguments": arguments or {}}, timeout, progress)

    def cancel(self, rid, reason="cancelled by the test"):
        self.send({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": rid, "reason": reason}})

    def progress(self, token):
        return [n["params"] for n in self.all_notifications
                if n.get("method") == "notifications/progress" and n["params"].get("progressToken") == token]

    def close(self):
        try:
            self.proc.stdin.close()
        except OSError:
            pass
        try:
            return self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            return self.proc.wait()


def main(argv):
    command = None
    if argv and argv[0] == "--server":
        end = argv.index("--")
        command, argv = argv[1:end], argv[end + 1:]
    modern = "--modern" in argv
    argv = [a for a in argv if a != "--modern"]
    client = McpClient(command, modern=modern)
    try:
        if modern:
            print(json.dumps(client.request("server/discover"), indent=2), file=sys.stderr)
        else:
            client.initialize()
        if not argv or argv[0] == "tools":
            for tool in client.tools():
                print(f"{tool['name']:20} {tool['description']}")
        elif argv[0] == "call":
            arguments = json.loads(argv[2]) if len(argv) > 2 else {}
            rid = client.start("tools/call", {"name": argv[1], "arguments": arguments}, progress="cli")
            while True:
                message = client.response(rid, timeout=0.5)
                for p in client.progress("cli"):
                    print(f"  … {p.get('message', '')}", file=sys.stderr)
                client.all_notifications.clear()
                if message is not None:
                    break
            if "error" in message:
                raise McpError(message["error"])
            result = message["result"]
            for block in result.get("content", []):
                if block["type"] == "text":
                    print(block["text"])
                else:
                    print(f"[{block['type']} {block.get('mimeType', '')}, {len(block.get('data', ''))} base64 characters]")
            return 1 if result.get("isError") else 0
        else:
            sys.exit(__doc__)
    finally:
        client.close()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
