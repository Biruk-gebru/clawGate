// mod declarations
mod proxy;
mod balancer;
mod middleware;

// crate imports
use crate::balancer::{SharedState, GateWayState};
use crate::proxy::proxy_request;
use crate::middleware::auth::require_auth;

// dependency imports
use axum::{
    Router,
};
use reqwest::Client;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use std::time::Duration;
use tower::limit::RateLimitLayer;


#[tokio::main]
async fn main() {
    //creating shared state
    let state = Arc::new(GateWayState {
        backends: vec![
            "http://127.0.0.1:4000".to_string(),
            "http://127.0.0.1:4001".to_string(),
        ],
        counter: AtomicUsize::new(0),
        client: Client::new(),
    });
    
    tracing_subscriber::fmt().init();
    //route create 
    let app = Router::new()
        .fallback(proxy_request)
        .with_state(state)
        .layer(middleware::from_fn(require_auth))
        .layer(RateLimitLayer::new(100, Duration::from_secs(1)))//chore:: make this global rate limiting specific to an ip
        .layer(TraceLayer::new_for_http());

    let listner = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listner, app).await.unwrap();
}
