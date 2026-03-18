use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use reqwest::Client;
use std::sync::RwLock;

use crate::dashboard::{SharedDashboard, CircuitState};
use crate::config::BalancingMode;

pub type SharedState = Arc<GateWayState>;
//A struct to hold the state of the gateway
pub struct GateWayState {
    pub backends: Arc<RwLock<Vec<String>>>,
    pub counter: AtomicUsize,//to avoid data race
    pub client: Client,//to have a single client at start up for all connections 
    pub dashboard: SharedDashboard,//contain the logs
    pub balancing: BalancingMode,
}

impl GateWayState {
    pub fn next_backend(&self) -> Option<String> {
        let backends = self.backends.read().unwrap();
        let dash = self.dashboard.lock().unwrap();
        let balancing = self.balancing;

        // Filter: eligible = not manually disabled AND circuit not Open
        let healthy_backends: Vec<&String> = backends.iter().filter(|url| {
            dash.backends.iter()
                .find(|b| &b.url == *url)
                .map(|b| {
                    !b.manually_disabled && match &b.circuit_state {
                        CircuitState::Closed => b.is_healthy,
                        CircuitState::Open { .. } => false,
                        CircuitState::HalfOpen => true,
                    }
                })
                .unwrap_or(true)
        }).collect();

        if healthy_backends.is_empty() {
            return None;
        }

        // Pin: if a backend is pinned, send all traffic there directly.
        // pinned_backend is an index into dash.backends (the full unfiltered list).
        if let Some(pin_idx) = dash.pinned_backend {
            if let Some(b) = dash.backends.get(pin_idx) {
                let eligible = !b.manually_disabled
                    && !matches!(b.circuit_state, CircuitState::Open { .. });
                if eligible {
                    return Some(b.url.clone());
                }
                // Pinned backend is down — fall through to normal round-robin
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
                    .min_by_key(|url|{
                        dash.backends.iter()
                            .find(|b| &b.url == **url)
                            .map(|b| b.active_connections.load(Ordering::Relaxed))
                            .unwrap_or(i64::MAX)
                    })
                    .map(|url| url.to_string())
            }
        }
    }
}
