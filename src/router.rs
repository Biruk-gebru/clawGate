use crate::config::RouteConfig;
use axum::http::HeaderMap;

/// Checks if a request path matches a route pattern (exact, glob prefix, or catch-all).
pub fn matches_path(pattern: &str, path: &str) -> bool {
    match pattern {
        "*" => true,
        p if p == path => true,
        p if p.ends_with("/*") => {
            let prefix = &p[..p.len() - 1]; // keep the trailing /
            path.starts_with(prefix) || path == &p[..p.len() - 2]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HeaderMatch;

    #[test]
    fn catch_all_matches_everything() {
        assert!(matches_path("*", "/"));
        assert!(matches_path("*", "/api/users"));
        assert!(matches_path("*", "/anything/at/all"));
    }

    #[test]
    fn exact_match() {
        assert!(matches_path("/health", "/health"));
        assert!(!matches_path("/health", "/healthz"));
        assert!(!matches_path("/health", "/health/deep"));
    }

    #[test]
    fn glob_prefix_match() {
        assert!(matches_path("/api/*", "/api/users"));
        assert!(matches_path("/api/*", "/api/orders/123"));
        assert!(!matches_path("/api/*", "/static/file.js"));
        assert!(!matches_path("/api/*", "/apiary"));
    }

    #[test]
    fn no_match_returns_false() {
        assert!(!matches_path("/api/*", "/static/file.js"));
        assert!(!matches_path("/health", "/"));
    }

    #[test]
    fn find_route_prefers_specificity() {
        let patterns = vec![
            "*".to_string(),
            "/api/*".to_string(),
            "/api/users".to_string(),
        ];
        assert_eq!(find_route(&patterns, "/api/users"), Some("/api/users"));
        assert_eq!(find_route(&patterns, "/api/orders"), Some("/api/*"));
        assert_eq!(find_route(&patterns, "/static/x"), Some("*"));
    }

    #[test]
    fn find_route_returns_none_when_empty() {
        let patterns: Vec<String> = vec![];
        assert_eq!(find_route(&patterns, "/anything"), None);
    }

    #[test]
    fn match_route_path_only() {
        let route = RouteConfig {
            match_pattern: Some("/api/*".to_string()),
            backends: vec![],
            match_header: None,
            split: None,
            label: None,
            ip_rules: None,
        };
        let headers = HeaderMap::new();
        assert!(match_route(&route, "/api/users", &headers));
        assert!(!match_route(&route, "/static/x", &headers));
    }

    #[test]
    fn match_route_header_and_path() {
        let route = RouteConfig {
            match_pattern: Some("/api/*".to_string()),
            backends: vec![],
            match_header: Some(HeaderMatch {
                name: "X-Version".to_string(),
                value: "v2".to_string(),
            }),
            split: None,
            label: None,
            ip_rules: None,
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-version", "v2".parse().unwrap());
        assert!(match_route(&route, "/api/users", &headers));

        headers.insert("x-version", "v1".parse().unwrap());
        assert!(!match_route(&route, "/api/users", &headers));
    }

    #[test]
    fn match_route_no_pattern_matches_any_path() {
        let route = RouteConfig {
            match_pattern: None,
            backends: vec![],
            match_header: None,
            split: None,
            label: None,
            ip_rules: None,
        };
        let headers = HeaderMap::new();
        assert!(match_route(&route, "/literally/anything", &headers));
    }
}
