mod proxy;
use crate::proxy::proxy_request;
use axum::{
    Router,
};

#[tokio::main]
async fn main() {
    //route create 
    let app = Router::new()
    .fallback(proxy_request);

    let listner = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listner, app).await.unwrap();
}
