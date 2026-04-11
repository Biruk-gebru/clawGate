# ClawGate

A reverse proxy and API gateway built in Rust with a live terminal dashboard.

ClawGate routes HTTP traffic across backend servers with health-aware load balancing, JWT authentication, canary deployments, and real-time observability through a TUI.

![ClawGate TUI Dashboard](doc/clawgate3.png)

## Features

- **Load balancing** - Round-robin, weighted, least connections, IP-hash sticky sessions
- **Routing** - Path-based, header-based, and canary/A-B traffic splitting per route
- **Health checks** - Background probes with automatic circuit breaker (Closed/Open/HalfOpen)
- **Security** - JWT auth (HS256), IP allowlist/denylist (per-route), per-IP rate limiting
- **Observability** - Prometheus metrics, structured JSON access logs, X-Request-ID propagation
- **TLS termination** - HTTPS via rustls with automatic cert reload
- **Hot-reload** - Edit `config.yaml` while running, changes apply instantly
- **Admin API** - REST endpoints to manage backends at runtime
- **Centralised config** - Optional etcd integration with graceful fallback to local YAML
- **Peer sync** - Gossip protocol (chitchat) shares health state across multiple instances
- **Persistence** - SQLite saves backend stats across restarts
- **Live TUI** - Real-time dashboard with per-backend stats, request log, and keyboard controls

## Quick Start

```bash
git clone https://github.com/Biruk-gebru/clawGate.git
cd clawGate/clawgate

# Start test backends
./scripts/backends.sh 4000,4001,4002

# Generate a JWT token
./scripts/gen_jwt.sh

# Run ClawGate
cargo run

# Send a request (use the token from gen_jwt.sh)
curl -H "Authorization: Bearer <token>" http://localhost:3000/api/users

# Press 'q' in the TUI to quit
# Stop backends
./scripts/stop_backends.sh
```

## Configuration

All configuration lives in `config.yaml`. See [doc/ARCHITECTURE.md](doc/ARCHITECTURE.md) for a full reference covering routing rules, load balancing algorithms, circuit breaker tuning, TLS setup, etcd integration, and gossip peer sync.

Minimal config:

```yaml
balancing: round_robin

backends:
  - url: "http://127.0.0.1:4000"
  - url: "http://127.0.0.1:4001"

routes:
  - match: "*"
    label: "default"
    backends:
      - url: "http://127.0.0.1:4000"
      - url: "http://127.0.0.1:4001"
```

## TUI Controls

| Key | Action |
|-----|--------|
| `←` / `→` | Select backend |
| `d` / `e` | Disable / enable backend |
| `p` / `u` | Pin / unpin traffic to backend |
| `Tab` | Switch tabs (Overview, Request Log, Config) |
| `/` | Search/filter request log |
| `q` | Quit |

## Project Structure

```
src/
  main.rs            Entry point
  proxy.rs           Request forwarding and metrics
  balancer.rs        Load balancing algorithms
  router.rs          Path and header matching
  config.rs          YAML config parsing + file watchers
  dashboard.rs       Shared runtime state
  health.rs          Health checks + circuit breaker
  rate_limiter.rs    Per-IP sliding window rate limiter
  persistence.rs     SQLite state save/restore
  etcd_config.rs     etcd backend config watcher
  gossip.rs          Chitchat gossip peer sync
  tui.rs             Terminal dashboard
  admin.rs           REST admin API
  middleware/
    auth.rs          JWT validation
    ip_rules.rs      IP allowlist/denylist
    request_id.rs    X-Request-ID injection
scripts/
  backends.sh        Start test backend servers
  stop_backends.sh   Stop test backends
  gen_jwt.sh         Generate a test JWT token
```
