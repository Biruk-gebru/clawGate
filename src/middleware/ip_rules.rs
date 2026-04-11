use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::IntoResponse;
use ipnetwork::IpNetwork;
use arc_swap::ArcSwap;

use crate::config::{IpRulesConfig, IpRulesMode};

/// Pre-parsed CIDR rules for IP allowlist/denylist filtering.
pub struct IpRules {
    pub mode: IpRulesMode,
    pub networks: Vec<IpNetwork>,
}

impl IpRules {
    pub fn from_config(cfg: &IpRulesConfig) -> Self {
        let networks = cfg.cidrs.iter()
            .filter_map(|s| {
                s.parse::<IpNetwork>()
                .map_err(|e| eprintln!("Bad CIDR '{}': {}", s, e))
                .ok()
            })
            .collect();
        IpRules {
            mode: cfg.mode.clone(),
            networks,
        }
    }

    pub fn is_allowed(&self, ip: IpAddr) -> bool {
        let matched = self.networks.iter().any(|n| n.contains(ip));
        match self.mode {
            IpRulesMode::Allowlist => matched,
            IpRulesMode::Denylist => !matched,
        }
    }
}

/// Middleware that checks the client IP against the current IP rules.
/// Increments the blocked counter on rejection.
pub async fn ip_filter(request: Request, next: Next, rules: Arc<ArcSwap<Option<IpRules>>>, blocked: Arc<AtomicU64>) -> impl IntoResponse {
    let loaded = rules.load();
    let Some(ref rules) = **loaded else {
        return next.run(request).await;
    };

    let frowarded_ip: Option<IpAddr> = request
        .headers()
        .get("X-Forwarded-For")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse().ok());
        
    let socket_ip: Option<IpAddr> = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());

    let mut client_ip = match frowarded_ip.or(socket_ip) {
        Some(ip) => ip,
        None => {return (StatusCode::INTERNAL_SERVER_ERROR, "Could not determine client IP").into_response();}
    };

    if let IpAddr::V6(v6) = client_ip {
        if let Some(v4) = v6.to_ipv4_mapped() {
            client_ip = IpAddr::V4(v4);
        }
    }

    if !rules.is_allowed(client_ip) {
        blocked.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::FORBIDDEN, "IP address not allowed").into_response();
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rules(mode: IpRulesMode, cidrs: &[&str]) -> IpRules {
        IpRules {
            mode,
            networks: cidrs.iter().filter_map(|s| s.parse().ok()).collect(),
        }
    }

    #[test]
    fn denylist_blocks_matching_ip() {
        let rules = make_rules(IpRulesMode::Denylist, &["192.168.1.0/24"]);
        let ip: IpAddr = "192.168.1.50".parse().unwrap();
        assert!(!rules.is_allowed(ip));
    }

    #[test]
    fn denylist_allows_non_matching_ip() {
        let rules = make_rules(IpRulesMode::Denylist, &["192.168.1.0/24"]);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(rules.is_allowed(ip));
    }

    #[test]
    fn allowlist_allows_matching_ip() {
        let rules = make_rules(IpRulesMode::Allowlist, &["10.0.0.0/8"]);
        let ip: IpAddr = "10.5.3.1".parse().unwrap();
        assert!(rules.is_allowed(ip));
    }

    #[test]
    fn allowlist_blocks_non_matching_ip() {
        let rules = make_rules(IpRulesMode::Allowlist, &["10.0.0.0/8"]);
        let ip: IpAddr = "192.168.1.1".parse().unwrap();
        assert!(!rules.is_allowed(ip));
    }

    #[test]
    fn multiple_cidrs() {
        let rules = make_rules(IpRulesMode::Denylist, &["192.168.1.0/24", "10.0.0.0/8"]);
        assert!(!rules.is_allowed("192.168.1.5".parse().unwrap()));
        assert!(!rules.is_allowed("10.5.0.1".parse().unwrap()));
        assert!(rules.is_allowed("172.16.0.1".parse().unwrap()));
    }

    #[test]
    fn empty_denylist_allows_everything() {
        let rules = make_rules(IpRulesMode::Denylist, &[]);
        assert!(rules.is_allowed("1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn empty_allowlist_blocks_everything() {
        let rules = make_rules(IpRulesMode::Allowlist, &[]);
        assert!(!rules.is_allowed("1.2.3.4".parse().unwrap()));
    }
}