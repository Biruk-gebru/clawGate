# ClawGate Architecture

## Overview

ClawGate is a reverse proxy that sits between clients and backend servers. Incoming requests flow through a middleware stack (rate limiting, auth, IP filtering, request ID injection), get matched to a route, and are forwarded to a healthy backend. A TUI dashboard renders real-time stats on the main thread while Axum serves requests in the background.

```
                  config.yaml ──(inotify)──> Config Watcher ──> mpsc channel
                                                                    │
                  etcd (optional) ──────────────────────────────────┤
                                                                    ▼
Client ──> Axum Router (port 3000) ──> Middleware Stack ──> proxy_request
           │                           │                        │
           │  TraceLayer               │  RateLimit             │  match_route()
           │  BufferLayer              │  IP Filter             │
           │  Auth (JWT)               │  Request ID            ▼
           │                           │                   RouteState
           │                           │                   (per-route balancer)
           │                           │                        │
           ▼                           ▼                        ▼
      Metrics endpoint            Admin API               Backend servers
      (port 9090)                 (port 9000)             :4000, :4001, ...
                                                                │
                                                                ▼
                                                        DashboardState
                                                        (Arc<Mutex>)
                                                                │
                          ┌─────────────────────────────────────┤
                          ▼                 ▼                   ▼
                    Health Checker     Gossip Peers         Ratatui TUI
                    (tokio task)       (chitchat UDP)       (main thread)
```

## Routing

Routes are evaluated top-to-bottom in `config.yaml`. First match wins. The `"*"` catch-all must be last.

### Path-based

```yaml
routes:
  - match: "/api/*"        # glob prefix
    backends:
      - url: "http://127.0.0.1:4000"
  - match: "/health"       # exact match
    backends:
      - url: "http://127.0.0.1:4001"
  - match: "*"             # catch-all
    backends:
      - url: "http://127.0.0.1:4002"
```

### Header-based

```yaml
routes:
  - match_header:
      name: "X-Version"
      value: "v2"
    backends:
      - url: "http://127.0.0.1:4010"
```

### Canary / A-B split

```yaml
routes:
  - match: "/api/*"
    split:
      - backends: ["http://127.0.0.1:4000"]
        weight: 90
      - backends: ["http://127.0.0.1:4001"]
        weight: 10
```

## Load Balancing

Set `balancing:` in config:

| Value | Behaviour |
|-------|-----------|
| `round_robin` | Default. Even distribution across healthy backends |
| `weighted_round_robin` | `weight: N` per backend. Higher weight = more traffic |
| `least_connections` | Picks the backend with fewest active connections |
| `ip_hash` | Same client IP always hits the same backend (sticky sessions) |

## Health Checks and Circuit Breaker

```yaml
health_check_interval_secs: 5

circuit_breaker:
  failure_threshold: 5
  cooldown: 30
```

A background tokio task pings each backend's health path every N seconds. After `failure_threshold` consecutive failures, the circuit trips to Open (no traffic). After `cooldown` seconds it enters HalfOpen and sends one probe request. Success closes the circuit; failure re-opens it.

| State | Traffic | Next transition |
|-------|---------|----------------|
| Closed | Normal | Trips to Open after N failures |
| Open | Blocked | Moves to HalfOpen after cooldown |
| HalfOpen | One probe | Success: Closed. Failure: Open |

## Security

### JWT Authentication

```yaml
auth:
  secret: "your-hmac-secret"
  required_claims: [sub, exp]
  issuer: "your-service"    # optional
```

All requests must include `Authorization: Bearer <token>`. Validates HS256 signature, expiry, issuer, and required claims.

### IP Rules

Global or per-route:

```yaml
ip_rules:
  mode: denylist            # or allowlist
  cidrs:
    - "192.168.1.0/24"

routes:
  - match: "/admin/*"
    ip_rules:
      mode: allowlist
      cidrs: ["10.0.0.0/8"]
    backends:
      - url: "http://127.0.0.1:4000"
```

Per-route rules override global rules. IPv6-mapped addresses (e.g. `::ffff:192.168.1.5`) are automatically normalised to IPv4 before matching.

### Rate Limiting

```yaml
rate_limit:
  requests: 100
  window_secs: 1
  per: ip
```

Sliding window per IP using DashMap. Stale entries are evicted every 60 seconds.

## TLS

```yaml
tls:
  cert_path: "./cert.pem"
  key_path: "./key.pem"
```

When TLS is configured, ClawGate binds with rustls on port 3000. Cert files are polled every 5 seconds and reloaded automatically on change. Generate test certs with:

```bash
openssl req -x509 -newkey rsa:2048 -keyout key.pem -out cert.pem -days 365 -nodes -subj "/CN=localhost"
```

## HTTP/2

```yaml
http2: true
```

Configures the reqwest client with HTTP/2 prior knowledge for upstream connections to backends.

## Observability

### Prometheus Metrics (port 9090)

- `request_count{backend, status}` - counter
- `request_duration{backend, status}` - histogram (ms)
- `active_connections{backend}` - gauge

### Access Logs

```yaml
access_log:
  path: "./access.log"
  enabled: true
```

One JSON line per request written via a background channel (non-blocking).

### Request ID

Every request gets an `X-Request-ID` header. If the client sends one, it's forwarded as-is. Otherwise a UUID v4 is generated.

## Admin API (port 9000)

```yaml
admin:
  port: 9000
  token: "your-admin-token"
```

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/admin/backends` | GET | List all backends with status |
| `/admin/backends` | POST | Add a backend at runtime |
| `/admin/backends/:url/disable` | POST | Disable a backend |
| `/admin/backends/:url/enable` | POST | Enable a backend |
| `/admin/drain` | POST | Graceful shutdown |

## Persistence

Backend stats (request_count, error_count, failed_count, circuit_state) are saved to `clawgate_state.db` (SQLite) when the TUI exits and restored on startup.

## Centralised Config (etcd)

```yaml
etcd:
  endpoint: "http://localhost:2379"
  key: "/clawgate/backends"
```

Watches the etcd key for changes and hot-reloads backends. Falls back gracefully to local YAML if etcd is unreachable. The value in etcd should be a YAML backend list:

```
[{url: "http://127.0.0.1:4000", weight: 3}, {url: "http://127.0.0.1:4001", weight: 1}]
```

## Gossip Peer Sync

```yaml
gossip:
  node_id: "node-1"
  listen_port: 7946
  seed_nodes: ["127.0.0.1:7947"]
```

Uses the chitchat crate (Raft-style gossip over UDP) to share backend health state across ClawGate instances. If any peer reports a backend as unhealthy, the local instance marks it unhealthy too (pessimistic merge).

To test with two instances, use separate config files with different gossip ports and point each instance's seed_nodes at the other.

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `axum` | Web framework and router |
| `tokio` | Async runtime |
| `reqwest` | HTTP client for backend forwarding |
| `tower` / `tower-http` | Middleware composition |
| `notify` | Filesystem watcher for hot-reload |
| `serde` / `serde_yaml` | Config parsing |
| `ratatui` / `crossterm` | Terminal UI |
| `jsonwebtoken` | JWT validation |
| `ipnetwork` | CIDR matching |
| `dashmap` | Concurrent per-IP rate limiting |
| `metrics` / `metrics-exporter-prometheus` | Prometheus export |
| `uuid` | Request ID generation |
| `axum-server` | TLS termination via rustls |
| `sqlx` | SQLite persistence |
| `etcd-client` | etcd config watcher |
| `chitchat` | Gossip peer sync |
| `arc-swap` | Lock-free hot-reload for IP rules |
