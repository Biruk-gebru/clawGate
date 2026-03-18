use axum::response::{IntoResponse, Response};
use axum::extract::Request as AxumRequest;
use axum::http::StatusCode;
use axum::body::Body;
use axum::extract::State;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;
use crate::balancer::SharedState;
use crate::dashboard::RequestLog;

struct ConnectionGuard(Arc<AtomicI64>);
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub async fn proxy_request(State(state): State<SharedState>, request: AxumRequest) -> impl IntoResponse {
    let client = &state.client;
    let start_time = Instant::now();

    let (parts, body) = request.into_parts();
    let method = parts.method;
    let uri = parts.uri;
    let headers = parts.headers;
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();

    // Get the next healthy backend — returns None if all are down
    let available_backend = match state.next_backend() {
        Some(b) => b,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "No healthy backends available").into_response(),
    };

    // Increment active connections for the chosen backend and clone the Arc for the RAII guard.
    // The guard automatically decrements when proxy_request returns (any path — Ok or Err).
    let conn_arc: Option<Arc<AtomicI64>> = {
        let mut dash = state.dashboard.lock().unwrap();
        dash.backends.iter_mut()
            .find(|b| b.url == available_backend)
            .map(|info| {
                info.active_connections.fetch_add(1, Ordering::Relaxed);
                Arc::clone(&info.active_connections)
            })
    };
    // Hold the guard for the lifetime of the request — Drop decrements the counter.
    let _guard = conn_arc.map(ConnectionGuard);

    // Save string versions BEFORE method/uri are moved into the reqwest call below
    let method_str = method.to_string();
    let path_str = uri.to_string();

    let target_uri = format!("{}{}", available_backend, uri);

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

    // Record metrics — scoped block so the lock drops before the .await below
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
    } // lock released here

    let body = response.bytes().await.unwrap();

    Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap()
        .into_response()
}