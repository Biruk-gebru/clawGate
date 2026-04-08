#!/usr/bin/env bash
set -euo pipefail

PIDFILE="/tmp/clawgate_backends.pids"

if [ ! -f "$PIDFILE" ]; then
  echo "No PID file found at $PIDFILE"
  exit 0
fi

while read -r pid; do
  if kill "$pid" 2>/dev/null; then
    echo "Stopped backend (pid $pid)"
  fi
done < "$PIDFILE"

rm -f "$PIDFILE"
echo "All backends stopped."
