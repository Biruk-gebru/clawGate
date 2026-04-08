#!/usr/bin/env bash
set -euo pipefail

PORTS="${1:-4000,4001,4002,4003}"
PIDFILE="/tmp/clawgate_backends.pids"

IFS=',' read -ra PORT_LIST <<< "$PORTS"

> "$PIDFILE"

for p in "${PORT_LIST[@]}"; do
  python3 -c "
import http.server, socketserver
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'backend:$p')
    def do_POST(self):
        self.do_GET()
    def log_message(self, *a):
        pass
with socketserver.TCPServer(('', $p), H) as s:
    s.serve_forever()
" &
  echo $! >> "$PIDFILE"
  echo "Started backend on :$p (pid $!)"
done

echo ""
echo "All backends running. PIDs written to $PIDFILE"
echo "Stop with: ./scripts/stop_backends.sh"
