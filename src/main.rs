mod proxy;
mod balancer;
mod middleware;
mod config;
mod dashboard;
mod tui;
mod health;
mod router;
mod rate_limiter;

use crate::balancer::{GateWayState, RouteState};
use crate::proxy::proxy_request;
use crate::middleware::ip_rules::{ip_filter, IpRules};
use crate::middleware::auth::require_auth;
use crate::config::{Config, BackendConfig, BalancingMode, RouteConfig, LogRecord};
use crate::dashboard::{BackendInfo, CircuitState, DashboardState, SharedDashboard};
use crate::rate_limiter::RateLimiter;
use crate::middleware::request_id::check_and_inject_request_id;
use axum::Router;
use axum::routing::get;
use axum::middleware::from_fn;
use axum::error_handling::HandleErrorLayer;
use reqwest::Client;
use reqwest::StatusCode;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI64, AtomicUsize};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc;
use tower_http::trace::TraceLayer;
use std::time::Duration;
use tower::limit::RateLimitLayer;
use tower::ServiceBuilder;
use tower::buffer::BufferLayer;
use tower::BoxError;


#[tokio::main]
async fn main() {
    // Tracing goes to stderr so it doesn't corrupt the TUI
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let config_data: Config = Config::load_config();
    let interval_secs = config_data.health_check_interval_secs.unwrap_or(5);
    let auth_cfg = Arc::new(config_data.auth.clone());

    let max_body_bytes = config_data.max_body_size_mb.map(|mb| (mb * 1024 * 1024) as usize);

    let initial_urls: Vec<String> = expand_backends(&config_data.backends, config_data.balancing);

    // Shared backend URL list (hot-swappable via config watcher)
    let backends_lock = Arc::new(RwLock::new(initial_urls.clone()));
    // Pre-parse CIDRs at startup so we don't parse per-request
    let ip_rules_arc = Arc::new(config_data.ip_rules.as_ref().map(IpRules::from_config));

    // Access log: create channel; spawn writer only if access_log is configured and enabled
    let (log_tx, mut log_rx) = tokio::sync::mpsc::channel::<LogRecord>(256);
    if let Some(ref al) = config_data.access_log {
        if al.enabled {
            let log_path = al.path.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let mut file: tokio::fs::File = tokio::fs::OpenOptions::new()
                    .create(true).append(true).open(&log_path).await
                    .expect("Failed to open access log");
                while let Some(record) = log_rx.recv().await {
                    if let Ok(mut line) = serde_json::to_string(&record) {
                        line.push('\n');
                        let _ = file.write_all(line.as_bytes()).await;
                    }
                }
            });
        }
    }


    // Dashboard must be built before routes so we can clone the Arc into each RouteState
    let dashboard: SharedDashboard = Arc::new(Mutex::new(DashboardState {
        backends: config_data.backends.iter().map(|b| {
            let label = config_data.routes.iter()
                .find(|r| r.backends.iter().any(|rb| rb.url == b.url))
                .map(|r| r.label.clone()
                    .unwrap_or_else(|| r.match_pattern.clone().unwrap_or_else(|| "*".to_string())))
                .unwrap_or_else(|| "default".to_string());

            BackendInfo {
                url: b.url.clone(),
                health_path: b.health_path.clone().unwrap_or_else(|| "/".to_string()),
                request_count: 0,
                weight: b.weight,
                last_hit: None,
                is_healthy: true,
                last_checked: None,
                circuit_state: CircuitState::Closed,
                failed_count: 0,
                manually_disabled: false,
                active_connections: Arc::new(AtomicI64::new(0)),
                route_label: label,
            }
        }).collect(),
        recent_request: VecDeque::new(),
        total_request: 0,
        status_msg: String::new(),
        health_check_interval_secs: interval_secs,
        selected_backend: 0,
        pinned_backend: None,
    }));

    // Build one RouteState per route; if no routes block, synthesise a catch-all.
    let routes: Vec<RouteState> = if config_data.routes.is_empty() {
        vec![RouteState {
            config: RouteConfig {
                match_pattern: Some("*".to_string()),
                backends: config_data.backends.clone(),
                match_header: None,
                split: None,
                label: None,
            },
            backends: Arc::clone(&backends_lock),
            counter: AtomicUsize::new(0),
            dashboard: Arc::clone(&dashboard),
        }]
    } else {
        config_data.routes.iter().map(|r| {
            let urls = expand_backends(&r.backends, config_data.balancing);
            RouteState {
                config: r.clone(),
                backends: Arc::new(RwLock::new(urls)),
                counter: AtomicUsize::new(0),
                dashboard: Arc::clone(&dashboard),
            }
        }).collect()
    };

    // Channel for config watcher
    let (tx, mut rx) = mpsc::channel::<Vec<BackendConfig>>(10);

    // Per-IP rate limiter (None if not configured)
    let rate_limiter: Option<Arc<RateLimiter>> = config_data.rate_limit.as_ref()
        .filter(|rl| rl.per == "ip")
        .map(|rl| Arc::new(RateLimiter::new(rl.requests, rl.window_secs)));

    // Shared gateway state
    let state = Arc::new(GateWayState {
        routes,
        client: Client::new(),
        global_dashboard: Arc::clone(&dashboard),
        balancing: config_data.balancing,
        rate_limiter: rate_limiter.clone(),
        max_body_bytes,
        log_tx: Some(log_tx),
    });

    // Start the health checker
    health::start_health_checker(
        Arc::clone(&dashboard),
        state.client.clone(),
        interval_secs,
        config_data.circuit_breaker.cooldown,
        config_data.circuit_breaker.failure_threshold,
    );

    // Purge stale IP rate-limit entries every 60s
    if let Some(limiter_arc) = rate_limiter {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                limiter_arc.evict_stale();
            }
        });
    }

    // Background task: hot-swaps backend lists when config changes
    let backends_for_updater = Arc::clone(&state.routes[0].backends);
    let dashboard_for_updater = Arc::clone(&dashboard);
    let balancing_mode_for_updater = config_data.balancing;
    tokio::spawn(async move {
        while let Some(new_backends) = rx.recv().await {
            let new_urls: Vec<String> = expand_backends(&new_backends, balancing_mode_for_updater);

            // Update the balancer's URL list
            {
                let mut backends = backends_for_updater.write().unwrap();
                *backends = new_urls.clone();
            }

            // Sync the dashboard so the TUI reflects the new config
            {
                let mut dash = dashboard_for_updater.lock().unwrap();

                for bc in &new_backends {
                    if let Some(existing) = dash.backends.iter_mut().find(|b| b.url == bc.url) {
                        existing.weight = bc.weight;
                        existing.health_path = bc.health_path.clone().unwrap_or_else(|| "/".to_string());
                    } else {
                        dash.backends.push(BackendInfo {
                            url: bc.url.clone(),
                            health_path: bc.health_path.clone().unwrap_or_else(|| "/".to_string()),
                            request_count: 0,
                            weight: bc.weight,
                            last_hit: None,
                            is_healthy: true,
                            last_checked: None,
                            circuit_state: CircuitState::Closed,
                            failed_count: 0,
                            manually_disabled: false,
                            active_connections: Arc::new(AtomicI64::new(0)),
                            route_label: "reloaded".to_string(),
                        });
                    }
                }

                // Remove backends that were dropped from config
                let unique_urls: Vec<&str> = new_backends.iter().map(|b| b.url.as_str()).collect();
                dash.backends.retain(|b| unique_urls.contains(&b.url.as_str()));

                dash.status_msg = format!("Config reloaded: {} backends active", new_backends.len());
            }
        }
    });

    // Watch config.yaml for changes
    Config::start_watcher("config.yaml", tx);

    let app = Router::new()
        .fallback(proxy_request)
        .with_state(state)
        .layer(from_fn(move |req, next| {
            check_and_inject_request_id(req, next)
        }))
        .layer(from_fn(move |req, next| {
            ip_filter(req, next, Arc::clone(&ip_rules_arc))
        }))
        .layer(from_fn(move |req, next| {
            require_auth(req, next, Arc::clone(&auth_cfg))
        }))
        .layer(ServiceBuilder::new()
            .layer(HandleErrorLayer::new(|_: BoxError| async {StatusCode::SERVICE_UNAVAILABLE}))
            .layer(BufferLayer::new(1024))
            .layer(RateLimitLayer::new(100, Duration::from_secs(1))))
        .layer(TraceLayer::new_for_http());

    let record_handle = metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder().expect("Failed to install Prometheus recorder");
        
    let metrics_app = Router::new()
        .route("/metrics", get(move || async move { record_handle.render() }));
    let metrics_listener = tokio::net::TcpListener::bind("0.0.0.0:9090").await.unwrap();
    tokio::spawn(async move {
        axum::serve(metrics_listener, metrics_app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();

    // Axum runs in background so the TUI can own the main thread
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });

    // TUI blocks until the user presses 'q'
    tui::run_tui(dashboard).expect("TUI crashed");
}

/// Expands a backend list into a rotation Vec, repeating each URL `weight` times.
/// With mode=RoundRobin all weights are ignored and each URL appears exactly once.
/// With mode=WeightedRoundRobin a backend with weight=3 gets 3 slots in the rotation.
pub fn expand_backends(backends: &[BackendConfig], mode: BalancingMode) -> Vec<String> {
    match mode {
        BalancingMode::RoundRobin | BalancingMode::LeastConnections | BalancingMode::IpHash => {
            backends.iter().map(|b| b.url.clone()).collect()
        }
        BalancingMode::WeightedRoundRobin => backends
            .iter()
            .flat_map(|b| std::iter::repeat(b.url.clone()).take(b.weight as usize))
            .collect(),
    }
}
