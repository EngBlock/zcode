#!/usr/bin/env python3
"""A release feed for the update check's tests (src/updates.rs): serves the
files of a folder over HTTP on a port of its own and logs each request.

    python3 scripts/fake_update_feed.py --dir <folder> --port-file /tmp/port --log /tmp/requests.jsonl

Put `appcast.json` (scripts/package.py appcast writes one) and the zip it
names in the folder; zcode reads the feed from
ZCODE_UPDATE_FEED=http://127.0.0.1:<port>/appcast.json. Nothing reaches a
real server.
"""

import argparse
import functools
import json
import os
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dir", required=True)
    parser.add_argument("--port-file", required=True)
    parser.add_argument("--log", required=True)
    args = parser.parse_args()

    class Handler(SimpleHTTPRequestHandler):
        def log_message(self, format, *values):  # noqa: A002
            with open(args.log, "a") as f:
                f.write(json.dumps({"method": self.command, "path": self.path, "status": values[1] if len(values) > 1 else None}) + "\n")

    server = ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=args.dir))
    with open(args.port_file + ".tmp", "w") as f:
        f.write(str(server.server_address[1]))
    os.rename(args.port_file + ".tmp", args.port_file)
    server.serve_forever()


if __name__ == "__main__":
    main()
