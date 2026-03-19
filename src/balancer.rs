use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use reqwest::Client;
use std::sync::RwLock;

use crate::dashboard::{SharedDashboard, CircuitState};
use crate::config::BalancingMode;

use rustc_hash::FxHasher;
use std::hash::{Hash, Hasher};

// Each route owns its own backend pool, round-robin counter, and a view into the dashboard
// so it can check health / circuit state for the backends that belong to it.
pub struct RouteState {
    pub pattern: String,
    pub backends: Arc<RwLock<Vec<String>>>,   // expanded URL list for THIS route
    pub counter: AtomicUsize,                  // independent round-robin counter per route
    pub dashboard: SharedDashboard,            // shared with health checker & proxy for metrics
}

impl RouteState {
    // Pick the next healthy backend from this route's pool.
    // Mirrors the old GateWayState::next_backend, but scoped to one route.
    pub fn next_backend(&self, client_ip: &str, balancing: BalancingMode) -> Option<String> {
        let backends = self.backends.read().unwrap();
        let dash = self.dashboard.lock().unwrap();

        // Filter: eligible = not manually disabled AND circuit not Open
        let healthy_backends: Vec<&String> = backends.iter().filter(|url| {
            dash.backends.iter()
                .find(|b| &b.url == *url)
                .map(|b| {
                    !b.manually_disabled && match &b.circuit_state {
                        CircuitState::Closed   => b.is_healthy,
                        CircuitState::Open { .. } => false,
                        CircuitState::HalfOpen => true,
                    }
                })
                .unwrap_or(true)  // unknown backend = allow (health checker will catch it)
        }).collect();

        if healthy_backends.is_empty() {
            return None;
        }

        // Pin: if a backend is pinned, send all traffic there directly.
        if let Some(pin_idx) = dash.pinned_backend {
            if let Some(b) = dash.backends.get(pin_idx) {
                let eligible = !b.manually_disabled
                    && !matches!(b.circuit_state, CircuitState::Open { .. });
                if eligible {
                    return Some(b.url.clone());
                }
                // Pinned backend is down — fall through to normal balancing
            }
        }

        match balancing {
            BalancingMode::RoundRobin | BalancingMode::WeightedRoundRobin => {
                let index = self.counter.fetch_add(1, Ordering::Relaxed) % healthy_backends.len();
                Some(healthy_backends[index].clone())
            }
            BalancingMode::LeastConnections => {
                healthy_backends
                    .iter()
                    .min_by_key(|url| {
                        dash.backends.iter()
                            .find(|b| &b.url == **url)
                            .map(|b| b.active_connections.load(Ordering::Relaxed))
                            .unwrap_or(i64::MAX)
                    })
                    .map(|url| url.to_string())
            }
            BalancingMode::IpHash => {
                let mut hasher = FxHasher::default();
                client_ip.hash(&mut hasher);
                let index = (hasher.finish() as usize) % healthy_backends.len();
                Some(healthy_backends[index].clone())
            }
        }
    }
}

pub type SharedState = Arc<GateWayState>;

// GateWayState is now just the router — it holds all route pools and the shared HTTP client.
// It no longer owns a single backend list or counter; those live inside each RouteState.
pub struct GateWayState {
    pub routes: Vec<RouteState>,
    pub client: Client,
    pub global_dashboard: SharedDashboard, // for TUI totals / recent request log
    pub balancing: BalancingMode,
}
