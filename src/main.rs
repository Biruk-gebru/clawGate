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
use axum::middleware::from_fn;
use axum::error_handling::HandleErrorLayer;
use reqwest::Client;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use std::time::Duration;
use tower::limit::RateLimitLayer;
use tower::ServiceBuilder;
use tower::buffer::BufferLayer;
use tower::BoxError;
use reqwest::StatusCode;



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
        .layer(from_fn(require_auth))
        .layer(ServiceBuilder::new()  
            .layer(HandleErrorLayer::new(|_: BoxError| async {StatusCode::SERVICE_UNAVAILABLE}))
            .layer(BufferLayer::new(1024))//add a buffer so axum gets a Clone type
            .layer(RateLimitLayer::new(1, Duration::from_secs(1))))//chore:: make this global rate limiting specific to an ip)
        .layer(TraceLayer::new_for_http());

    let listner = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listner, app).await.unwrap();
}
