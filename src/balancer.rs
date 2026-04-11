use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use reqwest::Client;
use std::sync::RwLock;
use tokio::sync::mpsc;

use crate::dashboard::{SharedDashboard, CircuitState};
use crate::config::{BalancingMode, RouteConfig};
use crate::rate_limiter::RateLimiter;
use crate::config::LogRecord;
use crate::middleware::ip_rules::IpRules;

use rustc_hash::FxHasher;
use std::hash::{Hash, Hasher};

/// Per-route state: owns a backend pool, round-robin counter, and dashboard reference.
pub struct RouteState {
    pub config: RouteConfig,
    pub backends: Arc<RwLock<Vec<String>>>,
    pub counter: AtomicUsize,
    pub dashboard: SharedDashboard,
    pub ip_rules: Option<IpRules>,
}

impl RouteState {
    /// Pick the next healthy backend from this route's pool.
    pub fn next_backend(&self, client_ip: &str, balancing: BalancingMode) -> Option<String> {
        let backends = self.backends.read().unwrap();
        let dash = self.dashboard.lock().unwrap();

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
                .unwrap_or(true)
        }).collect();

        if healthy_backends.is_empty() {
            return None;
        }

        if let Some(pin_idx) = dash.pinned_backend {
            if let Some(b) = dash.backends.get(pin_idx) {
                let eligible = !b.manually_disabled
                    && !matches!(b.circuit_state, CircuitState::Open { .. });
                if eligible {
                    return Some(b.url.clone());
                }
                // Pinned backend is down, fall through to normal balancing
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

/// Shared reference to the gateway state, passed to every request handler.
pub type SharedState = Arc<GateWayState>;

/// Top-level gateway state shared across all routes and request handlers.
pub struct GateWayState {
    pub routes: Vec<RouteState>,
    pub client: Client,
    pub global_dashboard: SharedDashboard,
    pub balancing: BalancingMode,
    pub rate_limiter: Option<Arc<RateLimiter>>,
    pub max_body_bytes: Option<usize>,
    pub log_tx : Option<mpsc::Sender<LogRecord>>,
}
