use axum::response::{IntoResponse, Response};
use axum::extract::Request as AxumRequest;
use axum::http::StatusCode;
use axum::body::Body;
use axum::extract::State;
use crate::balancer::SharedState;

pub async fn proxy_request(
        State(state): State<SharedState>,
        request: AxumRequest) -> impl IntoResponse {
    let client = &state.client;

    let (parts, body) = request.into_parts();
    let method = parts.method;
    let uri = parts.uri;
    let headers = parts.headers;
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();//added here to convert axum body to bytes so reqwest can handle it
    
    let available_backend = state.next_backend();
    let target_uri = format!("{}{}", available_backend, uri);

    let proxy_request = client.request(method, target_uri)
        .headers(headers)
        .body(bytes)
        .send()
        .await
        .map_err(|e| e.to_string()
    );
    
    let response = match proxy_request {
        Ok(response) => response,
        Err(e) => return (StatusCode::BAD_GATEWAY, e).into_response(),
    };
    
    let status = response.status();
    let body = response.bytes().await.unwrap();

    Response::builder()
    .status(status)
    .body(Body::from(body))
    .unwrap()
    .into_response()



}