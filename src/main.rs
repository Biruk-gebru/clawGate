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
    tracing_subscriber::fmt().init();

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
    }));

    // Channel for config watcher 
    let (tx, mut rx) = mpsc::channel(10);

    // Shared gateway state (passed into every request handler via Axum State)
    let state = Arc::new(GateWayState {
        backends: config,
        counter: AtomicUsize::new(0),
        client: Client::new(),
        dashboard: Arc::clone(&dashboard),
    });

    // Background task: receives new backend lists from the config watcher
    // and hot-swaps them into the shared state
    let backends_for_updater = Arc::clone(&state.backends);
    tokio::spawn(async move {
        while let Some(new_backends) = rx.recv().await {
            let mut backends = backends_for_updater.write().unwrap();
            *backends = new_backends;
            println!("Backends updated: {} active", backends.len());
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

    // Spawn axum server as a background task — it must not block main()
    // because the TUI needs to run on the main thread below
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // TUI runs on the main thread and blocks here until the user presses 'q'
    tui::run_tui(dashboard).expect("TUI crashed");
}
