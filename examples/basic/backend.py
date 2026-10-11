"""In-memory JSON API: GET /items lists items, POST /items adds one."""

import json
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = 8000
items = []


class Handler(BaseHTTPRequestHandler):
    def _send_json(self, status, body):
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        if self.path != "/items":
            return self._send_json(404, {"error": "not found"})
        self._send_json(200, items)

    def do_POST(self):
        if self.path != "/items":
            return self._send_json(404, {"error": "not found"})
        length = int(self.headers.get("Content-Length", 0))
        item = json.loads(self.rfile.read(length))
        items.append(item)
        print(f"added item: {item}")
        self._send_json(201, item)


if __name__ == "__main__":
    print(f"backend listening on http://localhost:{PORT}")
    HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
