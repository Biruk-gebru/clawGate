// mod declarations
mod proxy;
mod balancer;
mod middleware;
mod config;
mod dashboard;
mod tui;
mod health;
mod router;
mod rate_limiter;

// crate imports
use crate::balancer::{GateWayState, RouteState};
use crate::proxy::proxy_request;
use crate::middleware::ip_rules::{ip_filter, IpRules};
use crate::middleware::auth::require_auth;
use crate::config::{Config, BackendConfig, BalancingMode, RouteConfig};
use crate::dashboard::{BackendInfo, CircuitState, DashboardState, SharedDashboard};
use crate::rate_limiter::RateLimiter;
use crate::middleware::request_id::check_and_inject_request_id;

// dependency imports
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
    // Route tracing logs to stderr so they don't corrupt the TUI's stdout
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    // Load the full config struct (not just .backends — we need interval_secs too)
    let config_data: Config = Config::load_config();
    let interval_secs = config_data.health_check_interval_secs.unwrap_or(5);
    let auth_cfg = Arc::new(config_data.auth.clone());

    let max_body_bytes = config_data.max_body_size_mb.map(|mb| (mb * 1024 * 1024) as usize);

    // Weighted round-robin URLs
    let initial_urls: Vec<String> = expand_backends(&config_data.backends, config_data.balancing);

    // Shared backend URL list (hot-swappable via config watcher)
    // NOTE: in Phase 8, each RouteState has its OWN backends lock.
    // We keep one per route; for a config with no `routes:` block we
    // synthesise a single catch-all route from the top-level `backends:`.
    let backends_lock = Arc::new(RwLock::new(initial_urls.clone()));
    // Build IpRules from config (pre-parse CIDRs at startup, not per-request).
    // ip_rules is Option<IpRulesConfig>; we map over it so the Arc holds Option<IpRules>.
    let ip_rules_arc = Arc::new(config_data.ip_rules.as_ref().map(IpRules::from_config));

    // Build prometheus metrics
    let _recorder = metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder();

    // Build dashboard — must come BEFORE routes so we can clone the Arc into each RouteState.
    // For each backend, find which route it belongs to so we can display the label in the TUI.
    let dashboard: SharedDashboard = Arc::new(Mutex::new(DashboardState {
        backends: config_data.backends.iter().map(|b| {
            // Find the first route that contains this backend URL
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

    // Build one RouteState per route in config.
    // If no `routes:` block exists, synthesise a catch-all route from the top-level `backends:`.
    let routes: Vec<RouteState> = if config_data.routes.is_empty() {
        // Backward-compat: no routes block → single catch-all using top-level backends
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

    // Channel for config watcher — carries Vec<BackendConfig> now
    let (tx, mut rx) = mpsc::channel::<Vec<BackendConfig>>(10);

    // 9C — build per-IP limiter from config; None if block absent or per != "ip"
    let rate_limiter: Option<Arc<RateLimiter>> = config_data.rate_limit.as_ref()
        .filter(|rl| rl.per == "ip")
        .map(|rl| Arc::new(RateLimiter::new(rl.requests, rl.window_secs)));

    // Shared gateway state (passed into every request handler via Axum State)
    let state = Arc::new(GateWayState {
        routes,
        client: Client::new(),
        global_dashboard: Arc::clone(&dashboard),
        balancing: config_data.balancing,
        rate_limiter: rate_limiter.clone(),
        max_body_bytes,
    });

    // Start the health checker AFTER state is created so we can clone state.client
    health::start_health_checker(
        Arc::clone(&dashboard),
        state.client.clone(),
        interval_secs,
        config_data.circuit_breaker.cooldown,
        config_data.circuit_breaker.failure_threshold,
    );

    // 9C — purge stale IP entries every 60s
    if let Some(limiter_arc) = rate_limiter {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                limiter_arc.evict_stale();
            }
        });
    }

    // Background task: hot-swaps backend lists from the config watcher.
    // In Phase 8 we only handle the catch-all route (index 0) for hot-reload;
    // full per-route hot-reload can be wired in a later pass.
    let backends_for_updater = Arc::clone(&state.routes[0].backends);
    let dashboard_for_updater = Arc::clone(&dashboard);
    let balancing_mode_for_updater = config_data.balancing;
    tokio::spawn(async move {
        while let Some(new_backends) = rx.recv().await {
            let new_urls: Vec<String> = expand_backends(&new_backends, balancing_mode_for_updater);

            // 1. Update the balancer's URL list
            {
                let mut backends = backends_for_updater.write().unwrap();
                *backends = new_urls.clone();
            }

            // 2. Sync the dashboard's backend list so the TUI redraws correctly
            {
                let mut dash = dashboard_for_updater.lock().unwrap();

                // Add any new backends (preserve hit counts for existing ones)
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

                // Remove backends deleted from config.yaml (check against unique URLs, not expanded list)
                let unique_urls: Vec<&str> = new_backends.iter().map(|b| b.url.as_str()).collect();
                dash.backends.retain(|b| unique_urls.contains(&b.url.as_str()));

                dash.status_msg = format!("Config reloaded: {} backends active", new_backends.len());
            }
        }
    });

    // Start watching config.yaml for changes (runs on a background std::thread)
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
        })) // re-enable for production
        .layer(ServiceBuilder::new()
            .layer(HandleErrorLayer::new(|_: BoxError| async {StatusCode::SERVICE_UNAVAILABLE}))
            .layer(BufferLayer::new(1024))//add a buffer so axum gets a Clone type
            .layer(RateLimitLayer::new(100, Duration::from_secs(1))))//chore:: make this global rate limiting specific to an ip)
        .layer(TraceLayer::new_for_http());

    let metrics_app = Router::new()
        .route("/metrics", get(|| async { "Metrics" }));
    let metrics_listener = tokio::net::TcpListener::bind("0.0.0.0:9090").await.unwrap();
    tokio::spawn(async move {
        axum::serve(metrics_listener, metrics_app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();

    // Axum runs as a background task so the TUI can own the main thread
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });

    // TUI runs on the main thread and blocks here until the user presses 'q'
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
