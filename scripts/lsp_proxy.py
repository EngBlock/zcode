#!/usr/bin/env python3
"""A stand-in for sourcekit-lsp for the e2e tests (`ZCODE_SOURCEKIT_LSP`).

Runs the real sourcekit-lsp with the same arguments and passes messages
through both ways, and:
  * appends the method of every message zcode sends (one per line, with
    `$/cancelRequest` for cancellations) to `$ZCODE_LSP_PROXY_LOG`, so a
    test can count requests;
  * removes the capabilities named in `$ZCODE_LSP_PROXY_HIDE` (comma
    separated, e.g. `inlayHintProvider,callHierarchyProvider`) from the
    server's `initialize` answer, to check zcode does without them;
  * with `$ZCODE_LSP_PROXY_INLINE_TEXT`, offers `inlineCompletionProvider`
    (sourcekit-lsp has none) and answers `textDocument/inlineCompletion`
    itself with one item inserting that text at the position;
  * appends the params of every `textDocument/didOpen`, `didChange` and
    `didClose` zcode sends, as a JSON line, to `$ZCODE_LSP_PROXY_CHANGES`,
    so a test can replay what the server was told;
  * appends the uri and number of diagnostics of every
    `textDocument/publishDiagnostics` the server sends, as a JSON line, to
    `$ZCODE_LSP_PROXY_DIAGNOSTICS`.
"""

import json
import os
import subprocess
import sys
import threading

LOG = os.environ.get("ZCODE_LSP_PROXY_LOG")
HIDE = [c for c in os.environ.get("ZCODE_LSP_PROXY_HIDE", "").split(",") if c]
INLINE_TEXT = os.environ.get("ZCODE_LSP_PROXY_INLINE_TEXT")
CHANGES = os.environ.get("ZCODE_LSP_PROXY_CHANGES")
DIAGNOSTICS = os.environ.get("ZCODE_LSP_PROXY_DIAGNOSTICS")
initialize_ids = set()
log_lock = threading.Lock()
# Both threads write to zcode.
out_lock = threading.Lock()


def read_message(stream):
    length = None
    while True:
        line = stream.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            if length is not None:
                break
            continue
        name, _, value = line.partition(b":")
        if name.lower() == b"content-length":
            length = int(value)
    return json.loads(stream.read(length))


def write_message(stream, message):
    body = json.dumps(message).encode()
    with out_lock:
        stream.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        stream.flush()


def client_to_server(server):
    while (message := read_message(sys.stdin.buffer)) is not None:
        method = message.get("method")
        if method == "initialize":
            initialize_ids.add(message.get("id"))
        if LOG and method:
            with log_lock, open(LOG, "a") as log:
                log.write(method + "\n")
        if CHANGES and method in ("textDocument/didOpen", "textDocument/didChange", "textDocument/didClose"):
            with log_lock, open(CHANGES, "a") as log:
                log.write(json.dumps({"method": method, "params": message["params"]}) + "\n")
        if INLINE_TEXT is not None and method == "textDocument/inlineCompletion":
            items = [{"insertText": INLINE_TEXT}] if INLINE_TEXT else []
            write_message(sys.stdout.buffer, {"jsonrpc": "2.0", "id": message["id"], "result": {"items": items}})
            continue
        write_message(server.stdin, message)
    server.stdin.close()


def server_to_client(server):
    while (message := read_message(server.stdout)) is not None:
        if "method" not in message and message.get("id") in initialize_ids:
            capabilities = message.get("result", {}).get("capabilities", {})
            for name in HIDE:
                capabilities.pop(name, None)
            if INLINE_TEXT is not None:
                capabilities["inlineCompletionProvider"] = True
        if DIAGNOSTICS and message.get("method") == "textDocument/publishDiagnostics":
            with log_lock, open(DIAGNOSTICS, "a") as log:
                log.write(json.dumps({"uri": message["params"]["uri"], "count": len(message["params"]["diagnostics"])}) + "\n")
        write_message(sys.stdout.buffer, message)


def main():
    binary = subprocess.run(["xcrun", "--find", "sourcekit-lsp"], capture_output=True, text=True, check=True).stdout.strip()
    server = subprocess.Popen([binary, *sys.argv[1:]], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    threading.Thread(target=client_to_server, args=(server,), daemon=True).start()
    server_to_client(server)
    server.wait()


if __name__ == "__main__":
    main()
