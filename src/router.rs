use crate::config::RouteConfig;
use axum::http::HeaderMap;

pub fn matches_path(pattern: &str, path: &str) -> bool {
    match pattern {
        "*" => true,                                     // catch-all
        p if p == path => true,                          // exact match
        p if p.ends_with("/*") => {
            // prefix glob: "/api/*" matches "/api/users", "/api/orders"
            let prefix = &p[..p.len() - 2];             // strip the "/*"
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

/// Returns true if the incoming request matches the route's rules.
/// Both path and header conditions must pass (AND logic).
/// If a condition is absent on the route config, it is treated as a wildcard (always passes).
pub fn match_route(route: &RouteConfig, path: &str, headers: &HeaderMap) -> bool {
    // Path check — if match_pattern is None it means "any path" (header-only route)
    let path_ok = match &route.match_pattern {
        Some(p) => matches_path(p, path),
        None => true,
    };

    // Header check — if match_header is None it means "any headers"
    let header_ok = match &route.match_header {
        Some(hm) => headers
            .get(hm.name.to_lowercase().as_str())  // HeaderMap stores names in lowercase
            .and_then(|v| v.to_str().ok())          // convert HeaderValue → &str (may fail if non-UTF8)
            .map(|v| v == hm.value)                 // compare to expected value
            .unwrap_or(false),                      // absent header = no match
        None => true,
    };

    path_ok && header_ok
}
