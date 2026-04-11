use axum::http::HeaderValue;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::IntoResponse;

/// Middleware that ensures every request has an X-Request-ID header.
/// Uses the existing header if present, otherwise generates a UUID v4.
pub async fn check_and_inject_request_id(mut request: Request, next: Next) -> impl IntoResponse {
    let headers = request.headers();
    let id = headers.get("X-Request-ID").map(|v| v.to_str().unwrap().to_string());
    if id.is_none() {
        let id = uuid::Uuid::new_v4().to_string();
        let mut headers = request.headers_mut();
        headers.insert("X-Request-ID", HeaderValue::from_str(&id).unwrap());
    }
    next.run(request).await
}