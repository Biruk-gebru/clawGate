use std::collections::VecDeque;
use std::time::Instant;
use std::sync::{Arc, Mutex};

pub struct BackendInfo {
    pub url: String,
    pub request_count: u64,
    pub last_hit: Option<Instant>,
}

pub struct RequestLog {
    pub method: String,
    pub path: String, 
    pub backends: String,
    pub status: u16,
    pub duration_ms: u128,
}

pub struct DashboardState {
    pub backends: Vec<BackendInfo>,
    pub recent_request: VecDeque<RequestLog>,
    pub total_request: u64,
}

pub type SharedDashboard = Arc<Mutex<DashboardState>>; 