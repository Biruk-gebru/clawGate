//understadning axum and creating a simple server with a single route
use axum::{
    routing::get,
    Router,
};

#[tokio::main]
async fn main() {
    //route create 
    let app = Router::new().route("/", get(|| async { "Hello, World!"}));

    let listner = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listner, app).await.unwrap();
}
