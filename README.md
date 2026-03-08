# 🦀 ClawGate

A lightweight API gateway built in Rust, featuring round-robin load balancing, middleware layers, hot-reloadable configuration, and a live terminal dashboard.

![ClawGate TUI Dashboard](../doc/clawgate.png)

---

## Features

| Feature | Description |
|---------|-------------|
| **Pass-through proxy** | Forwards all incoming HTTP requests to backend servers |
| **Round-robin load balancing** | Distributes requests evenly across all configured backends |
| **Middleware stack** | Logging (TraceLayer), authentication, and rate limiting |
| **Hot-reload config** | Edit `config.yaml` while the gateway is running — backends update instantly, no restart needed |
| **Live TUI dashboard** | Terminal UI showing per-server hit counts, active request routing, and a scrolling request log |

---

## Requirements

- [Rust](https://rustup.rs/) (stable, 1.75+)
- Cargo (comes with Rust)

---

## Quick Start

### 1. Clone the repo

```bash
git clone https://github.com/Biruk-gebru/clawGate.git
cd clawGate/clawgate
```

### 2. Configure your backends

Edit `config.yaml` to point to your backend servers:

```yaml
backends:
  - "http://127.0.0.1:4000"
  - "http://127.0.0.1:4001"
  - "http://127.0.0.1:4002"
```

You can add or remove entries here **while the gateway is running** and the change takes effect immediately.

### 3. Start your backend servers

For quick testing, use Python's built-in HTTP server:

```bash
# In separate terminals:
python3 -m http.server 4000
python3 -m http.server 4001
python3 -m http.server 4002
```

### 4. Run ClawGate

```bash
RUST_LOG=info cargo run
```

The TUI dashboard launches automatically. The gateway listens on **port 3000**.

### 5. Send requests

```bash
curl http://localhost:3000/
```

Watch the TUI — the server boxes light up green as each request hits a backend, and the request log updates in real time.

### 6. Quit

Press **`q`** inside the TUI to shut down the gateway.

---

## The TUI Dashboard

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 🦀 ClawGate  |  Backends: 3  |  Total Requests: 42  |  Press 'q' to quit   │
└─────────────────────────────────────────────────────────────────────────────┘
┌ 🖥  :4000 ──────────┐┌ 🖥  :4001 ──────────┐┌ 🖥  :4002 ──────────┐
│  Hits: 14           ││  Hits: 14           ││  Hits: 14           │
│  ● ACTIVE           ││    idle             ││    idle             │
└─────────────────────┘└─────────────────────┘└─────────────────────┘
┌ Recent Requests ────────────────────────────────────────────────────────────┐
│ Method   Path          Backend                  Status   Time (ms)          │
│ GET      /             http://127.0.0.1:4000    200      3ms                │
│ GET      /favicon.ico  http://127.0.0.1:4001    404      2ms                │
│ GET      /style.css    http://127.0.0.1:4002    200      5ms                │
└─────────────────────────────────────────────────────────────────────────────┘
```

| Element | Description |
|---------|-------------|
| **Title bar** | Total backend count, total request count, and config reload events |
| **Server boxes** | One box per backend. Turns **green** for 300ms after a request hits it |
| **Hits counter** | Cumulative request count per backend since startup |
| **Recent Requests** | Scrolling log of the last 20 requests — method, path, backend, HTTP status (colour-coded), and response time |

---

## Hot-Reload Configuration

While ClawGate is running, open `config.yaml` and add or remove backend URLs, then save. The gateway detects the change via `inotify` (Linux) and:

1. Updates the load balancer's routing list immediately
2. Adds new server boxes to the TUI dashboard
3. Removes boxes for deleted backends (hit counts for surviving backends are preserved)
4. Shows a confirmation message in the TUI title bar

```yaml
# Add a new backend while running:
backends:
  - "http://127.0.0.1:4000"
  - "http://127.0.0.1:4001"
  - "http://127.0.0.1:4002"
  - "http://127.0.0.1:4003"   # ← new backend appears instantly in the TUI
```

---

## Middleware

The middleware stack runs in this order for every request:

```
Incoming request
  → TraceLayer        (logs method, URI, status, latency)
  → RateLimitLayer    (100 requests/sec global limit, returns 503 when buffer overflows)
  → BufferLayer       (queues up to 1024 requests to make rate limiter Clone-safe)
  → require_auth      (checks for Authorization header, returns 401 if missing)
  → proxy_request     (core proxy + metrics recording)
```

### Enabling Authentication

Authentication is disabled by default for development convenience. To enable it, uncomment this line in `main.rs`:

```rust
.layer(from_fn(require_auth))
```

Then all requests must include an `Authorization` header:

```bash
curl -H "Authorization: Bearer your-token" http://localhost:3000/
```

---

## Project Structure

```
clawgate/
├── config.yaml              # Backend list — edit this at runtime for hot-reload
├── Cargo.toml
└── src/
    ├── main.rs              # Entry point — wires everything together
    ├── proxy.rs             # Core request forwarding + metrics recording
    ├── balancer.rs          # GatewayState, shared state, round-robin selection
    ├── config.rs            # config.yaml loader + inotify file watcher
    ├── dashboard.rs          # Shared metrics state (BackendInfo, RequestLog)
    ├── tui.rs               # Ratatui TUI — render loop and all widgets
    └── middleware/
        ├── mod.rs
        └── auth.rs          # Authorization header check middleware
```

---

## Architecture

```
                  ┌──────────────────────────────────┐
                  │           config.yaml             │
                  └─────────────┬────────────────────┘
                                │ inotify (notify crate)
                                ▼
                  ┌──────────────────────────────────┐
                  │         Config Watcher            │
                  │   (std::thread + mpsc sender)     │
                  └─────────────┬────────────────────┘
                                │ Vec<String> over channel
                                ▼
┌───────────┐    ┌──────────────────────────────────┐    ┌─────────────────┐
│  Client   │───▶│          Axum Router              │───▶│  Backend :4000  │
│ :PORT     │    │  TraceLayer                       │    ├─────────────────┤
└───────────┘    │  RateLimitLayer                   │───▶│  Backend :4001  │
                 │  BufferLayer                      │    ├─────────────────┤
                 │  require_auth (optional)          │───▶│  Backend :4002  │
                 │  proxy_request ──▶ GatewayState   │    └─────────────────┘
                 └──────────────────┬───────────────┘
                                    │ Arc<Mutex<DashboardState>>
                                    ▼
                 ┌──────────────────────────────────┐
                 │         Ratatui TUI               │
                 │  (main thread, redraws @ 20 FPS)  │
                 └──────────────────────────────────┘
```

---

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `axum` | Web framework and router |
| `tokio` | Async runtime |
| `reqwest` | HTTP client for forwarding requests |
| `tower` | Middleware composition (Buffer, RateLimit) |
| `tower-http` | HTTP-specific middleware (TraceLayer) |
| `notify` | Filesystem watcher for config hot-reload |
| `serde` + `serde_yaml` | YAML config parsing |
| `ratatui` | Terminal UI framework |
| `crossterm` | Cross-platform terminal control |
| `tracing` + `tracing-subscriber` | Structured logging |

---

## Phases Built

This project was built incrementally across 5 phases:

1. **Phase 1 — Pass-through Proxy**: Basic Axum server that forwards all requests to a single backend
2. **Phase 2 — Load Balancing**: Round-robin distribution across multiple backends using `Arc<AtomicUsize>`
3. **Phase 3 — Middleware Stack**: Logging, rate limiting, and JWT-style auth header validation
4. **Phase 4 — Dynamic Config**: Hot-reload backends from `config.yaml` using `notify` + `tokio::sync::mpsc`
5. **Phase 5 — TUI Dashboard**: Live terminal UI with per-server metrics and request log using `ratatui`
