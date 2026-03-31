use std::sync::Arc;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::IntoResponse;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use crate::config::AuthConfig;

type Claims = std::collections::HashMap<String, serde_json::Value>;

pub async fn require_auth(
    request: Request,
    next: Next,
    auth_cfg: Arc<Option<AuthConfig>>,
) -> impl IntoResponse {
    // No auth configured, pass through
    let Some(auth) = auth_cfg.as_ref() else {
        return next.run(request).await;
    };

    let Some(header) = request.headers().get("Authorization") else {
        return (StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response();
    };

    let token_str = match header.to_str() {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, "Malformed Authorization header").into_response(),
    };

    let Some(token) = token_str.strip_prefix("Bearer ") else {
        return (StatusCode::UNAUTHORIZED, "Authorization must be 'Bearer <token>'").into_response();
    };

    let key = DecodingKey::from_secret(auth.secret.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    if let Some(iss) = &auth.issuer {
        validation.set_issuer(&[iss.as_str()]);
    }

    let token_data = match decode::<Claims>(token, &key, &validation) {
        Ok(data) => data,
        Err(e) => return (StatusCode::UNAUTHORIZED, format!("Invalid token: {}", e)).into_response(),
    };

    if let Some(required) = &auth.required_claims {
        for claim in required {
            if !token_data.claims.contains_key(claim) {
                return (
                    StatusCode::UNAUTHORIZED,
                    format!("Missing required claim: {}", claim),
                ).into_response();
            }
        }
    }

    next.run(request).await
}
