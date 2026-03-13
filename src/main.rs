// mod declarations
mod proxy;
mod balancer;
mod middleware;
mod config;
mod dashboard;
mod tui;
mod health;

// crate imports
use crate::balancer::GateWayState;
use crate::proxy::proxy_request;
use crate::middleware::auth::require_auth;
use crate::config::{Config, BackendConfig};
use crate::dashboard::{BackendInfo, CircuitState, DashboardState, SharedDashboard};

// dependency imports
use axum::Router;
use axum::middleware::from_fn;
use axum::error_handling::HandleErrorLayer;
use reqwest::Client;
use reqwest::StatusCode;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
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

    // The balancer only needs URLs — extract them from BackendConfig
    let initial_urls: Vec<String> = config_data.backends.iter().map(|b| b.url.clone()).collect();

    // Shared backend URL list (hot-swappable via config watcher)
    let backends_lock = Arc::new(RwLock::new(initial_urls.clone()));

    // Build dashboard — initialize BackendInfo from the loaded config
    let dashboard: SharedDashboard = Arc::new(Mutex::new(DashboardState {
        backends: config_data.backends.iter().map(|b| BackendInfo {
            url: b.url.clone(),
            health_path: b.health_path.clone().unwrap_or_else(|| "/".to_string()),
            request_count: 0,
            last_hit: None,
            is_healthy: true,          // assume healthy until first check
            last_checked: None,
            circuit_state: CircuitState::Closed,
            failed_count: 0,
        }).collect(),
        recent_request: VecDeque::new(),
        total_request: 0,
        status_msg: String::new(),
        health_check_interval_secs: interval_secs,
    }));

    // Channel for config watcher — carries Vec<BackendConfig> now
    let (tx, mut rx) = mpsc::channel::<Vec<BackendConfig>>(10);

    // Shared gateway state (passed into every request handler via Axum State)
    let state = Arc::new(GateWayState {
        backends: Arc::clone(&backends_lock),
        counter: AtomicUsize::new(0),
        client: Client::new(),
        dashboard: Arc::clone(&dashboard),
    });

    // Start the health checker AFTER state is created so we can clone state.client
    health::start_health_checker(
        Arc::clone(&dashboard),
        state.client.clone(),
        interval_secs,
        config_data.circuit_breaker.cooldown,
        config_data.circuit_breaker.failure_threshold,
    );

    // Background task: receives new backend lists from the config watcher
    // and hot-swaps them into BOTH the balancer state AND the dashboard
    let backends_for_updater = Arc::clone(&state.backends);
    let dashboard_for_updater = Arc::clone(&dashboard);
    tokio::spawn(async move {
        while let Some(new_backends) = rx.recv().await {
            let new_urls: Vec<String> = new_backends.iter().map(|b| b.url.clone()).collect();

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
                    if !dash.backends.iter().any(|b| b.url == bc.url) {
                        dash.backends.push(BackendInfo {
                            url: bc.url.clone(),
                            health_path: bc.health_path.clone().unwrap_or_else(|| "/".to_string()),
                            request_count: 0,
                            last_hit: None,
                            is_healthy: true,
                            last_checked: None,
                            circuit_state: CircuitState::Closed,
                            failed_count: 0,
                        });
                    }
                }

                // Remove backends deleted from config.yaml
                dash.backends.retain(|b| new_urls.contains(&b.url));

                dash.status_msg = format!("Config reloaded: {} backends active", new_urls.len());
            }
        }
    });

    // Start watching config.yaml for changes (runs on a background std::thread)
    Config::start_watcher("config.yaml", tx);

    let app = Router::new()
        .fallback(proxy_request)
        .with_state(state)
        //.layer(from_fn(require_auth)) // re-enable for production
        .layer(ServiceBuilder::new()
            .layer(HandleErrorLayer::new(|_: BoxError| async {StatusCode::SERVICE_UNAVAILABLE}))
            .layer(BufferLayer::new(1024))//add a buffer so axum gets a Clone type
            .layer(RateLimitLayer::new(100, Duration::from_secs(1))))//chore:: make this global rate limiting specific to an ip)
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();

    // Axum runs as a background task so the TUI can own the main thread
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // TUI runs on the main thread and blocks here until the user presses 'q'
    tui::run_tui(dashboard).expect("TUI crashed");
}
