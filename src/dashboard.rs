use std::collections::VecDeque;
use std::time::Instant;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicI64;
use serde::Serialize;

#[derive(Clone, Debug)]
pub enum CircuitState {
    Closed,
    Open { tripped_at: Instant },
    HalfOpen,
}

impl PartialEq for CircuitState {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (CircuitState::Closed, CircuitState::Closed)
            | (CircuitState::Open { .. }, CircuitState::Open { .. })
            | (CircuitState::HalfOpen, CircuitState::HalfOpen)
        )
    }
}

pub struct BackendInfo {
    pub url: String,
    pub weight: u32,
    pub request_count: u64,
    pub last_hit: Option<Instant>,
    pub health_path: String,
    pub is_healthy: bool,
    pub last_checked: Option<Instant>,
    pub failed_count: u64,
    pub circuit_state: CircuitState,
    pub manually_disabled: bool,
    pub active_connections: Arc<AtomicI64>,
    pub route_label: String,
}

pub struct RequestLog {
    pub method: String,
    pub path: String, 
    pub backends: String,
    pub status: u16,
    pub duration_ms: u128,
    pub request_id: String,
}

pub struct DashboardState {
    pub backends: Vec<BackendInfo>,
    pub recent_request: VecDeque<RequestLog>,
    pub total_request: u64,
    pub status_msg: String,
    pub health_check_interval_secs: u64,
    pub selected_backend: usize,
    pub pinned_backend: Option<usize>,
}

#[derive(Serialize)]
pub struct BackendDto {
    pub url: String,
    pub healthy: bool,
    pub manually_disabled: bool,
    pub request_count: u64,
    pub active_connections: i64,
    pub route_label: String,
}


pub type SharedDashboard = Arc<Mutex<DashboardState>>; 