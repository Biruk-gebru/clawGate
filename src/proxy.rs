use axum::response::{IntoResponse, Response};
use axum::extract::Request as AxumRequest;
use axum::http::StatusCode;
use axum::body::Body;
use axum::extract::State;
use std::time::Instant;
use crate::balancer::SharedState;


pub async fn proxy_request(State(state): State<SharedState>,request: AxumRequest) -> impl IntoResponse {
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

    let duration = start_time.elaspsed().as_millis();
    let mut dash = state.dashboard.lock().unwrap();

    if let Some(backend_info) = dash.backends.iter_mut().find(|b| b.url == available_backend) {
        backend_info.request_count += 1;
        backend_info.last_hit = Some(Instant::now());
    }

    dash.recent_request.push_front(RequestLog {
        method: method.to_string(),
        path: uri.to_string(),
        backends: available_backend,
        status: status.as_u16(),
        duration_ms: duration,
    });

    if dash.recent_request.len() > 10 {
        dash.recent_request.pop_back();
    }
    
    let status = response.status();
    let body = response.bytes().await.unwrap();

    drop(dash);//releasing lock 

    Response::builder()
    .status(status)
    .body(Body::from(body))
    .unwrap()
    .into_response()



}