#!/usr/bin/env python3
"""Static file server for the repo root, for the browser host.

    python3 tools/serve.py            # http://127.0.0.1:8765/tools/web/
    python3 tools/serve.py 9000       # pick a port

Serves with the right MIME types for .wasm/.mjs and no caching, so an edit to
tools/web/* shows up on reload.
"""
import functools
import http.server
import os
import socketserver
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".mjs": "text/javascript",
        ".js": "text/javascript",
        ".svg": "image/svg+xml",
    }

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, fmt, *args):
        sys.stderr.write("%s\n" % (fmt % args))


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    socketserver.TCPServer.allow_reuse_address = True
    handler = functools.partial(Handler, directory=ROOT)
    with socketserver.TCPServer(("127.0.0.1", port), handler) as httpd:
        print(f"serving {ROOT} at http://127.0.0.1:{port}/tools/web/", flush=True)
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
