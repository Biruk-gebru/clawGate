// mod declarations
mod proxy;
mod balancer;
mod middleware;
mod config;

// crate imports
use crate::balancer::GateWayState;
use crate::proxy::proxy_request;
use crate::middleware::auth::require_auth;
use crate::config::Config;

// dependency imports
use axum::{
    Router,
};
use axum::middleware::from_fn;
use axum::error_handling::HandleErrorLayer;
use reqwest::Client;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tower_http::trace::TraceLayer;
use std::time::Duration;
use tower::limit::RateLimitLayer;
use tower::ServiceBuilder;
use tower::buffer::BufferLayer;
use tower::BoxError;
use reqwest::StatusCode;



#[tokio::main]
async fn main() {
    let config = Arc::new(RwLock::new(Config::load_config().backends));
    let (tx, mut rx) = mpsc::channel(10);

    //creating shared state
    let state = Arc::new(GateWayState {
        backends: config,
        counter: AtomicUsize::new(0),
        client: Client::new(),
    });

    let backends_for_updater = Arc::clone(&state.backends);
    tokio::spawn(async move {
        while let Some(new_backends) = rx.recv().await {
            let mut backends = backends_for_updater.write().unwrap();
            *backends = new_backends;
            println!("Backends updated: {} ", backends.len());
        }
    });

    Config::start_watcher("config.yaml", tx);
    
    tracing_subscriber::fmt().init();
    //route create 
    let app = Router::new()
        .fallback(proxy_request)
        .with_state(state)
        //.layer(from_fn(require_auth)) //for dev simlicity 
        .layer(ServiceBuilder::new()  
            .layer(HandleErrorLayer::new(|_: BoxError| async {StatusCode::SERVICE_UNAVAILABLE}))
            .layer(BufferLayer::new(1024))//add a buffer so axum gets a Clone type
            .layer(RateLimitLayer::new(100, Duration::from_secs(1))))//chore:: make this global rate limiting specific to an ip)
        .layer(TraceLayer::new_for_http());

    let listner = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("Server started on port 3000");
    axum::serve(listner, app).await.unwrap();
}
