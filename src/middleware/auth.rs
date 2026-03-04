use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::IntoResponse;

pub async fn require_auth(request: Request, next: Next) -> impl IntoResponse {
    let auth_header = request.headers().get("Authorization");

    match auth_header {
        Some(header) => {
            next.run(request).await
        }
        None => {
            StatusCode::UNAUTHORIZED.into_response()
        }
    }
}

