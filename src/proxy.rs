use axum::response::{IntoResponse, Response};
use axum::extract::Request as AxumRequest;
use axum::http::StatusCode;
use axum::body::Body;
use axum::extract::State;
use std::time::Instant;
use crate::balancer::SharedState;
use crate::dashboard::RequestLog;

pub async fn proxy_request(State(state): State<SharedState>, request: AxumRequest) -> impl IntoResponse {
    let client = &state.client;
    let start_time = Instant::now(); // start timing before we do any work

    let (parts, body) = request.into_parts();
    let method = parts.method;
    let uri = parts.uri;
    let headers = parts.headers;
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();//added here to convert axum body to bytes so reqwest can handle it

    let available_backend = state.next_backend();
    let target_uri = format!("{}{}", available_backend, uri);

    // Save string versions BEFORE method/uri are moved into the request below
    let method_str = method.to_string();
    let path_str = uri.to_string();

    let proxy_response = client.request(method, target_uri)
        .headers(headers)
        .body(bytes)
        .send()
        .await
        .map_err(|e| e.to_string());

    let response = match proxy_response {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, e).into_response(),
    };

    let duration = start_time.elapsed().as_millis();
    let status = response.status();

    // Record metrics
    {
        let mut dash = state.dashboard.lock().unwrap();

        if let Some(info) = dash.backends.iter_mut().find(|b| b.url == available_backend) {
            info.request_count += 1;
            info.last_hit = Some(Instant::now());
        }
        dash.total_request += 1;

        dash.recent_request.push_front(RequestLog {
            method: method_str,
            path: path_str,
            backends: available_backend,
            status: status.as_u16(),
            duration_ms: duration,
        });

        if dash.recent_request.len() > 20 {
            dash.recent_request.pop_back();
        }
    } // created to drop dash

    let body = response.bytes().await.unwrap();

    Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap()
        .into_response()
}