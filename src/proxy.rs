use axum::response::{IntoResponse, Response};
use axum::extract::Request as AxumRequest;
use axum::http::StatusCode;
use axum::body::Body;
use axum::extract::State;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;
use rand::RngExt;


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
    let headers = parts.headers;  // must be extracted BEFORE the route match that reads it
    let path = uri.path();

    let route_state = state.routes.iter().find(|r| crate::router::match_route(&r.config, path, &headers));

    let route = match route_state {
        Some(r) => r,
        None => return (StatusCode::NOT_FOUND, "No route matched").into_response(),
    };

    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let client_ip = headers.get("X-Forwarded-For").and_then(|e| e.to_str().ok()).unwrap_or("");

    // 8C — Canary / A-B split: if the matched route has a split config, use weighted random
    // selection to choose a backend group, then pick a URL from that group.
    // Otherwise fall back to the route's standard next_backend() selection.
    let available_backend: String = if let Some(split_groups) = &route.config.split {
        let total_weight: u32 = split_groups.iter().map(|g| g.weight).sum();
        assert!(total_weight > 0, "Split weights must sum to a positive number");

        let roll: u32 = rand::rng().random_range(0..total_weight);
        let mut cumulative = 0u32;
        let chosen_group = split_groups.iter().find(|g| {
            cumulative += g.weight;
            roll < cumulative
        }).expect("cumulative weight must cover roll — check split config");

        // Pick round-robin within the chosen group's URL list.
        // Simple: use the route's shared counter mod group size.
        let idx = route.counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % chosen_group.backends.len();
        chosen_group.backends[idx].clone()
    } else {
        // No split — use normal balancing (round-robin / least-conn / ip-hash)
        match route.next_backend(client_ip, state.balancing) {
            Some(b) => b,
            None => return (StatusCode::SERVICE_UNAVAILABLE, "No healthy backends available").into_response(),
        }
    };


    // Increment active connections for the chosen backend and clone the Arc for the RAII guard.
    // The guard automatically decrements when proxy_request returns (any path — Ok or Err).
    let conn_arc: Option<Arc<AtomicI64>> = {
        let mut dash = state.global_dashboard.lock().unwrap();
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
        let mut dash = state.global_dashboard.lock().unwrap();

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