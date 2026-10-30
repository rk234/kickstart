"""Serves an HTML page listing the items fetched from the backend."""

import json
import os
from html import escape
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.request import urlopen

PORT = int(os.environ.get("FRONTEND_PORT", 3000))
BACKEND = f"http://localhost:{os.environ.get('BACKEND_PORT', 8000)}"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        with urlopen(f"{BACKEND}/items") as resp:
            items = json.load(resp)
        rows = "".join(
            f"<li>{escape(item['name'])}: ${item['price']:.2f}</li>" for item in items
        )
        page = f"<!doctype html><title>Items</title><h1>Items</h1><ul>{rows}</ul>".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(page)))
        self.end_headers()
        self.wfile.write(page)


if __name__ == "__main__":
    print(f"frontend listening on http://localhost:{PORT}")
    HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
