mod proxy;
mod balancer;
use crate::balancer::{SharedState, GateWayState};
use crate::proxy::proxy_request;
use axum::{
    Router,
};
use reqwest::Client;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

    //route create 
    let app = Router::new()
    .fallback(proxy_request).with_state(state);

    let listner = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listner, app).await.unwrap();
}
