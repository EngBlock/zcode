#!/usr/bin/env python3
"""A stand-in for the Claude API's Messages endpoint, for the e2e tests and
for trying edit predictions without a key or network.

    python3 scripts/fake_anthropic.py --port-file /tmp/port --log /tmp/requests.jsonl \
        --key sk-ant-fake --reply /tmp/reply.json

Serves `POST /v1/messages` on 127.0.0.1 (a free port, written to
`--port-file`) and streams the answer as server-sent events the way the API
does (`message_start`, `content_block_start`, `ping`, `content_block_delta`
text deltas, `content_block_stop`, `message_delta`, `message_stop`), in
HTTP/1.1 chunks. The answer comes from `--reply`, a JSON file read for each
request, so a test can change it between requests:

    {"text": "name)",      what the model "writes" after <insert> (the
                           </insert> stop sequence is never sent, like the API)
     "chunk": 4,           characters per text delta
     "delay_ms": 30,       pause between deltas
     "hold_ms": 0,         pause before answering at all (a slow model)
     "status": 200,        anything else answers with that status and an
     "error_type": "...",  API error object instead
     "stream_error": false a mid-stream `error` event (overloaded) instead
                           of the end}

A request without `x-api-key` equal to `--key` (or $FAKE_ANTHROPIC_KEY) gets
a 401, as the API answers. Each request is appended to `--log` as JSON: the
body, the `anthropic-version` header, whether the key matched (never the key
itself), and whether the client hung up before the answer ended (a cancelled
prediction).
"""

import argparse
import json
import os
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

args = None
log_lock = threading.Lock()


def log(entry):
    if args.log:
        with log_lock, open(args.log, "a") as f:
            f.write(json.dumps(entry) + "\n")


def reply():
    try:
        with open(args.reply) as f:
            return json.load(f)
    except (OSError, ValueError, TypeError):
        return {"text": ""}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(length) or b"{}")
        key_ok = self.headers.get("x-api-key") == args.key
        entry = {
            "path": self.path,
            "version": self.headers.get("anthropic-version"),
            "key_ok": key_ok,
            "body": body,
            "time": time.time(),
        }
        if self.path != "/v1/messages":
            return self.error(404, "not_found_error", "Not found", entry)
        if not key_ok:
            return self.error(401, "authentication_error", "invalid x-api-key", entry)
        script = reply()
        if script.get("status", 200) != 200:
            return self.error(script["status"], script.get("error_type", "api_error"), script.get("message", "scripted error"), entry)
        if script.get("hold_ms"):
            time.sleep(script["hold_ms"] / 1000)
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("transfer-encoding", "chunked")
        self.send_header("cache-control", "no-cache")
        self.end_headers()
        text = "<insert>" + script.get("text", "")
        chunk = max(1, script.get("chunk", 4))
        delay = script.get("delay_ms", 30) / 1000
        try:
            self.event("message_start", {"type": "message_start", "message": {
                "id": "msg_fake", "type": "message", "role": "assistant", "model": body.get("model"),
                "content": [], "stop_reason": None, "usage": {"input_tokens": 100, "output_tokens": 1}}})
            self.event("content_block_start", {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})
            self.event("ping", {"type": "ping"})
            for i in range(0, len(text), chunk):
                time.sleep(delay)
                self.event("content_block_delta", {"type": "content_block_delta", "index": 0,
                                                   "delta": {"type": "text_delta", "text": text[i:i + chunk]}})
            if script.get("stream_error"):
                self.event("error", {"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})
            else:
                self.event("content_block_stop", {"type": "content_block_stop", "index": 0})
                self.event("message_delta", {"type": "message_delta", "delta": {"stop_reason": "stop_sequence", "stop_sequence": "</insert>"},
                                             "usage": {"output_tokens": len(text) // 4 + 1}})
                self.event("message_stop", {"type": "message_stop"})
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()
            entry["hung_up"] = False
        except (BrokenPipeError, ConnectionResetError):
            entry["hung_up"] = True
        log(entry)

    def event(self, name, data):
        payload = f"event: {name}\ndata: {json.dumps(data)}\n\n".encode()
        self.wfile.write(b"%x\r\n" % len(payload) + payload + b"\r\n")
        self.wfile.flush()

    def error(self, status, kind, message, entry):
        payload = json.dumps({"type": "error", "error": {"type": kind, "message": message}}).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
        entry["status"] = status
        log(entry)


def main():
    global args
    parser = argparse.ArgumentParser()
    parser.add_argument("--port-file")
    parser.add_argument("--log")
    parser.add_argument("--reply")
    parser.add_argument("--key", default=os.environ.get("FAKE_ANTHROPIC_KEY", "sk-ant-fake"))
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    if args.port_file:
        with open(args.port_file + ".tmp", "w") as f:
            f.write(str(server.server_address[1]))
        os.rename(args.port_file + ".tmp", args.port_file)
    else:
        print(server.server_address[1], flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
