use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use reqwest::Client;
use std::sync::RwLock;

use crate::dashboard::{SharedDashboard, CircuitState};

pub type SharedState = Arc<GateWayState>;
//A struct to hold the state of the gateway
pub struct GateWayState {
    pub backends: Arc<RwLock<Vec<String>>>,
    pub counter: AtomicUsize,//to avoid data race
    pub client: Client,//to have a single client at start up for all connections 
    pub dashboard: SharedDashboard,//contain the logs
}

impl GateWayState {
    pub fn next_backend(&self) -> Option<String> {
        let backends = self.backends.read().unwrap();
        let dash = self.dashboard.lock().unwrap();

        // Filter to only backends marked healthy (unknown = assume healthy so new backends work)
        let healthy_backends: Vec<&String> = backends.iter().filter(|url| {
            dash.backends.iter()
                .find(|b| &b.url == *url)
                .map(|b| match &b.circuit_state {
                    CircuitState::Closed => b.is_healthy,
                    CircuitState::Open { .. } => false,
                    CircuitState::HalfOpen => true,
                })   
                .unwrap_or(true)
        }).collect();

        if healthy_backends.is_empty() {
            return None;
        }

        // Round-robin over the HEALTHY list only
        let index = self.counter.fetch_add(1, Ordering::Relaxed) % healthy_backends.len();
        Some(healthy_backends[index].clone())
    }
}
