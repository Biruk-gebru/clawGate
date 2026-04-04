use axum::{
    Router,
    routing::{get, post},
    extract::{State, Path},
    Json,
    response::IntoResponse,
    http::{StatusCode, HeaderMap},
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use serde::Serialize;

use crate::dashboard::{SharedDashboard, BackendDto};

pub struct AdminState {
    pub dashboard: SharedDashboard,
    pub token: String,
}

pub type SharedAdminState = Arc<AdminState>;

// Helper: returns false if the Authorization: Bearer <token> header doesn't match.
fn check_token(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|t| t == token)
        .unwrap_or(false)
}

// Shared response shape for all backends
fn to_dto(backends: &crate::dashboard::DashboardState) -> Vec<BackendDto> {
    backends.backends.iter().map(|b| BackendDto {
        url: b.url.clone(),
        healthy: b.is_healthy,
        manually_disabled: b.manually_disabled,
        request_count: b.request_count,
        active_connections: b.active_connections.load(Ordering::Relaxed),
        route_label: b.route_label.clone(),
    }).collect()
}

// GET /admin/backends — list all backends + health + hit counts
async fn list_backends(
    State(state): State<SharedAdminState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_token(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response();
    }
    let dash = state.dashboard.lock().unwrap();
    Json(to_dto(&dash)).into_response()
}

// POST /admin/backends/:url/disable
async fn disable_backend(
    State(state): State<SharedAdminState>,
    headers: HeaderMap,
    Path(url): Path<String>,
) -> impl IntoResponse {
    if !check_token(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response();
    }
    let mut dash = state.dashboard.lock().unwrap();
    if let Some(b) = dash.backends.iter_mut().find(|b| b.url == url) {
        b.manually_disabled = true;
        return (StatusCode::OK, "Backend disabled").into_response();
    }
    (StatusCode::NOT_FOUND, "Backend not found").into_response()
}

// POST /admin/backends/:url/enable
async fn enable_backend(
    State(state): State<SharedAdminState>,
    headers: HeaderMap,
    Path(url): Path<String>,
) -> impl IntoResponse {
    if !check_token(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response();
    }
    let mut dash = state.dashboard.lock().unwrap();
    if let Some(b) = dash.backends.iter_mut().find(|b| b.url == url) {
        b.manually_disabled = false;
        return (StatusCode::OK, "Backend enabled").into_response();
    }
    (StatusCode::NOT_FOUND, "Backend not found").into_response()
}

// GET /admin/stats — total request count + per-backend hit counts
#[derive(Serialize)]
struct StatsResponse {
    total_requests: u64,
    backends: Vec<BackendDto>,
}

async fn get_stats(
    State(state): State<SharedAdminState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_token(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response();
    }
    let dash = state.dashboard.lock().unwrap();
    Json(StatsResponse {
        total_requests: dash.total_request,
        backends: to_dto(&dash),
    }).into_response()
}

pub fn admin_router(state: SharedAdminState) -> Router {
    Router::new()
        .route("/admin/backends", get(list_backends))
        .route("/admin/backends/{url}/disable", post(disable_backend))
        .route("/admin/backends/{url}/enable", post(enable_backend))
        .route("/admin/stats", get(get_stats))
        .with_state(state)
}