use crate::config::RouteConfig;
use axum::http::HeaderMap;

/// Checks if a request path matches a route pattern (exact, glob prefix, or catch-all).
pub fn matches_path(pattern: &str, path: &str) -> bool {
    match pattern {
        "*" => true,
        p if p == path => true,
        p if p.ends_with("/*") => {
            let prefix = &p[..p.len() - 2];
            path.starts_with(prefix)
        }
        _ => false,
    }
}

pub fn find_route<'a>(patterns: &'a [String], path: &str) -> Option<&'a str> {
    let mut best: Option<&str> = None;
    let mut best_len: usize = 0;

    for p in patterns {
        if matches_path(p, path) {
            let specificity = if p == "*" {
                0
            } else if p.ends_with("/*") {
                p.len() - 2
            } else {
                usize::MAX
            };

            if specificity > best_len || best.is_none() {
                best_len = specificity;
                best = Some(p.as_str());
            }
        }
    }
    best
}

/// Returns true if the request matches this route's path and header conditions (AND logic).
pub fn match_route(route: &RouteConfig, path: &str, headers: &HeaderMap) -> bool {
    let path_ok = match &route.match_pattern {
        Some(p) => matches_path(p, path),
        None => true,
    };

    let header_ok = match &route.match_header {
        Some(hm) => headers
            .get(hm.name.to_lowercase().as_str())
            .and_then(|v| v.to_str().ok())
            .map(|v| v == hm.value)
            .unwrap_or(false),
        None => true,
    };

    path_ok && header_ok
}
