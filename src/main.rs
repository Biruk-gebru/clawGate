// mod declarations
mod proxy;
mod balancer;
mod middleware;
mod config;
mod dashboard;
mod tui;

// crate imports
use crate::balancer::GateWayState;
use crate::proxy::proxy_request;
use crate::middleware::auth::require_auth;
use crate::config::Config;
use crate::dashboard::{BackendInfo, DashboardState, SharedDashboard};

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

    // Load initial config from yaml
    let initial_backends = Config::load_config().backends;

    // Shared backend list (hot-swappable via config watcher)
    let config = Arc::new(RwLock::new(initial_backends.clone()));

    // Build dashboard — initialize backend info from the loaded list
    let dashboard: SharedDashboard = Arc::new(Mutex::new(DashboardState {
        backends: initial_backends.iter().map(|url| BackendInfo {
            url: url.clone(),
            request_count: 0,
            last_hit: None,
        }).collect(),
        recent_request: VecDeque::new(),
        total_request: 0,
        status_msg: String::new(),
    }));

    // Channel for config watcher 
    let (tx, mut rx) = mpsc::channel::<Vec<String>>(10);

    // Shared gateway state (passed into every request handler via Axum State)
    let state = Arc::new(GateWayState {
        backends: config,
        counter: AtomicUsize::new(0),
        client: Client::new(),
        dashboard: Arc::clone(&dashboard),
    });

    // Background task: receives new backend lists from the config watcher
    // and hot-swaps them into BOTH the balancer state AND the dashboard
    let backends_for_updater = Arc::clone(&state.backends);
    let dashboard_for_updater = Arc::clone(&dashboard);
    tokio::spawn(async move {
        while let Some(new_backends) = rx.recv().await {
            // 1. Update the balancer's routing list
            {
                let mut backends = backends_for_updater.write().unwrap();
                *backends = new_backends.clone();
            }

            // 2. Sync the dashboard's backend list so the TUI redraws correctly
            {
                let mut dash = dashboard_for_updater.lock().unwrap();

                // Add any new backends (preserve hit counts for existing ones)
                for url in &new_backends {
                    if !dash.backends.iter().any(|b| &b.url == url) {
                        dash.backends.push(BackendInfo {
                            url: url.clone(),
                            request_count: 0,
                            last_hit: None,
                        });
                    }
                }

                // Remove backends that were deleted from config.yaml
                dash.backends.retain(|b| new_backends.contains(&b.url));

                // Update status message shown in TUI title bar (no println! — that corrupts the terminal)
                dash.status_msg = format!("Config reloaded: {} backends active", new_backends.len());
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
