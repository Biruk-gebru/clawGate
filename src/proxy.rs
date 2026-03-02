use axum::response::{IntoResponse, Response};
use axum::extract::Request as AxumRequest;
use reqwest::Client;
use axum::http::StatusCode;
use axum::body::Body;

pub async fn proxy_request(request: AxumRequest) -> impl IntoResponse {
    let client = Client::new();

    let (parts, body) = request.into_parts();
    let method = parts.method;
    let uri = parts.uri;
    let headers = parts.headers;
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();//added here to convert axum body to bytes so reqwest can handle it
    
    let target_uri = format!("http://127.0.0.1:4000{}", uri);

    let proxy_request = client.request(method, target_uri)
    .headers(headers)
    .body(bytes)
    .send()
    .await
    .map_err(|e| e.to_string());
    let response = match proxy_request {
        Ok(response) => response,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.unwrap();

    Response::builder()
    .status(status)
    .body(Body::from(body))
    .unwrap()
    .into_response()



}