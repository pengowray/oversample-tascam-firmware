"""Serve web/ for the emulator with caching turned off, so a rebuilt
WebAssembly module or edited page is always loaded fresh.

Usage: python3 web/serve.py [PORT]   (default 8742)
"""
import functools
import http.server
import os
import sys


class NoCache(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


port = int(sys.argv[1]) if len(sys.argv) > 1 else 8742
handler = functools.partial(NoCache, directory=os.path.dirname(os.path.abspath(__file__)))
http.server.ThreadingHTTPServer(("", port), handler).serve_forever()
