use axum::response::{IntoResponse, Response};
use axum::extract::Request as AxumRequest;
use axum::http::StatusCode;
use axum::body::Body;
use axum::extract::State;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;
use rand::RngExt;
use chrono::Utc;

use metrics::{gauge, counter, histogram};

use crate::balancer::SharedState;
use crate::config::LogRecord;
use crate::dashboard::RequestLog;

struct ConnectionGuard(Arc<AtomicI64>, String);
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
        gauge!("active_connections", "backend" => self.1.clone()).set(self.0.load(Ordering::Relaxed) as f64);
    }
}

pub async fn proxy_request(State(state): State<SharedState>, request: AxumRequest) -> impl IntoResponse {
    let client = &state.client;
    let start_time = Instant::now();

    let (parts, body) = request.into_parts();
    let method = parts.method;
    let uri = parts.uri;
    let headers = parts.headers;
    let path = uri.path();
    let request_id = headers
        .get("X-Request-ID")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    let route_state = state.routes.iter().find(|r| crate::router::match_route(&r.config, path, &headers));

    let route = match route_state {
        Some(r) => r,
        None => return (StatusCode::NOT_FOUND, "No route matched").into_response(),
    };

    if let Some(max) = state.max_body_bytes {
        if let Some(cl) = headers
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<usize>().ok())
        {
            if cl > max {
                return (StatusCode::PAYLOAD_TOO_LARGE, "Body too large").into_response();
            }
        }
    }

    let limit = state.max_body_bytes.unwrap_or(usize::MAX);
    let bytes = match axum::body::to_bytes(body, limit).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "Body too large").into_response(),
    };

    let client_ip = headers
        .get("X-Forwarded-For")
        .and_then(|e| e.to_str().ok())
        .unwrap_or("")
        .to_string();

    if let Some(ip_rules) = &route.ip_rules {
        if let Ok(ip) = client_ip.parse::<std::net::IpAddr>() {
            if !ip_rules.is_allowed(ip) {
                return (StatusCode::FORBIDDEN, "IP address not allowed").into_response();
            }
        }
    }

    // Per-IP rate limit (fails open if client_ip isn't parseable)
    if let Some(limiter) = &state.rate_limiter {
        if let Ok(ip_addr) = client_ip.parse::<std::net::IpAddr>() {
            if !limiter.check_and_record(ip_addr) {
                return (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded").into_response();
            }
        }
    }

    // Traffic splitting: weighted random group selection if split is configured,
    // otherwise standard load balancing.
    let available_backend: String = if let Some(split_groups) = &route.config.split {
        let total_weight: u32 = split_groups.iter().map(|g| g.weight).sum();
        assert!(total_weight > 0, "Split weights must sum to a positive number");

        let roll: u32 = rand::rng().random_range(0..total_weight);
        let mut cumulative = 0u32;
        let chosen_group = split_groups
            .iter()
            .find(|g| {
                cumulative += g.weight;
                roll < cumulative
            })
            .expect("cumulative weight must cover roll");

        let idx = route.counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % chosen_group.backends.len();
        chosen_group.backends[idx].clone()
    } else {
        match route.next_backend(&client_ip, state.balancing) {
            Some(b) => b,
            None => return (StatusCode::SERVICE_UNAVAILABLE, "No healthy backends available").into_response(),
        }
    };

    // Increment active connections; RAII guard decrements on return.
    let conn_arc: Option<Arc<AtomicI64>> = {
        let mut dash = state.global_dashboard.lock().unwrap();
        dash.backends
            .iter_mut()
            .find(|b| b.url == available_backend)
            .map(|info| {
                info.active_connections.fetch_add(1, Ordering::Relaxed);
                Arc::clone(&info.active_connections)
            })
    };
    let backend_label = available_backend.clone();
    gauge!("active_connections", "backend" => backend_label)
        .set(conn_arc.as_ref().map_or(0, |a| a.load(Ordering::Relaxed)) as f64);
    let _guard = conn_arc.map(|arc| ConnectionGuard(arc, available_backend.clone()));

    // Save owned copies before method/uri are moved into the reqwest call
    let method_str = method.to_string();
    let path_str = uri.to_string();

    let target_uri = format!("{}{}", available_backend, uri);

    let proxy_response = client
        .request(method, target_uri)
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

    counter!("request_count", "backend" => available_backend.clone(), "status" => status.to_string()).increment(1);
    histogram!("request_duration", "backend" => available_backend.clone(), "status" => status.to_string()).record(duration as f64);

    // Fire-and-forget: non-blocking send to background log writer.
    // try_send() drops the record silently if the channel is full — never stalls the request path.
    if let Some(log_tx) = &state.log_tx {
        let _ = log_tx.try_send(LogRecord {
            timestamp: Utc::now().to_rfc3339(),
            request_id: request_id.clone(),
            method: method_str.clone(),
            path: path_str.clone(),
            backend: available_backend.clone(),
            status: status.as_u16(),
            duration_ms: duration,
            client_ip: client_ip.clone(),
        });
    }

    // Record to TUI dashboard (scoped so the lock drops before the .await)
    {
        let mut dash = state.global_dashboard.lock().unwrap();

        if let Some(info) = dash.backends.iter_mut().find(|b| b.url == available_backend) {
            info.request_count += 1;
            info.last_hit = Some(Instant::now());
            // Track latency for sparkline (last 30 samples)
            info.latency_history.push_back(duration);
            if info.latency_history.len() > 30 {
                info.latency_history.pop_front();
            }
            // Track 5xx for error rate bar
            if status.as_u16() >= 500 {
                info.error_count += 1;
            }
        }
        dash.total_request += 1;

        dash.recent_request.push_front(RequestLog {
            method: method_str,
            path: path_str,
            backends: available_backend,
            status: status.as_u16(),
            duration_ms: duration,
            request_id,
        });

        if dash.recent_request.len() > 20 {
            dash.recent_request.pop_back();
        }
    }

    let body = response.bytes().await.unwrap();

    Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap()
        .into_response()
}