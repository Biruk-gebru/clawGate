use std::fs;
use tokio::sync::mpsc;
use notify::Watcher;
use std::path::Path;

#[derive(serde::Deserialize,Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IpRulesMode {
    Allowlist,
    Denylist,
}

#[derive(serde::Deserialize, Clone)]
pub struct IpRulesConfig {
    pub mode: IpRulesMode,
    pub cidrs: Vec<String>,
}

#[derive(serde::Deserialize, Clone)]
pub struct SplitGroupConfig {
    pub backends: Vec<String>,
    pub weight: u32,
}

#[derive(serde::Deserialize, Clone)]
pub struct HeaderMatch {
    pub name: String,   // the header name to match on, e.g. "X-Version"
    pub value: String,  // the expected value, e.g. "v2"
}

#[derive(serde::Deserialize, Clone)]
pub struct RouteConfig {
    #[serde(rename = "match")]
    pub match_pattern: Option<String>,
    #[serde(default)]
    pub backends: Vec<BackendConfig>,
    pub match_header: Option<HeaderMatch>,
    pub split: Option<Vec<SplitGroupConfig>>,  // for 8C canary: [{backends:[...], weight:90}, ...]
    pub label: Option<String>,                 // optional display name shown in TUI
}

#[derive(serde::Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum BalancingMode {
    #[default]
    RoundRobin,
    WeightedRoundRobin,
    LeastConnections,
    IpHash,
}

#[derive(serde::Deserialize, Clone)]
pub struct AuthConfig {
    pub secret: String,
    pub required_claims: Option<Vec<String>>,
    pub issuer: Option<String>,
}

#[derive(serde::Deserialize, Clone)]
pub struct BackendConfig {
    pub url: String,
    pub health_path: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: u32, // for weighted round robin; defaults to 1
}

#[derive(serde::Deserialize, Clone)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: u64,
    pub cooldown: u64,
}

#[derive(serde::Deserialize, Clone)]
pub struct RateLimitConfig {
    pub requests: u64,
    pub window_secs: u64,
    pub per: String,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        CircuitBreakerConfig { failure_threshold: 5, cooldown: 30 }
    }
}

fn default_weight() -> u32 { 1 }

#[derive(serde::Deserialize)]
pub struct Config {
    pub backends: Vec<BackendConfig>,
    pub health_check_interval_secs: Option<u64>,
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerConfig,
    pub auth: Option<AuthConfig>,
    #[serde(default)]
    pub balancing: BalancingMode,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
    pub ip_rules: Option<IpRulesConfig>,
    pub rate_limit: Option<RateLimitConfig>,
}

impl Config {
    pub fn load_config() -> Config {
        let path = "config.yaml";
        let content = fs::read_to_string(path).expect("Failed to read config");
        serde_yaml::from_str(&content).expect("Failed to parse config")
    }

    pub fn start_watcher(path: &str, sender: mpsc::Sender<Vec<BackendConfig>>) {
        let path = path.to_string(); // path is a &str — just convert directly, no .url
        let path_clone = path.clone();

        std::thread::spawn(move || {
            let sender_clone = sender.clone();

            let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                match result {
                    Ok(event) => {
                        if let notify::EventKind::Modify(_) = event.kind {
                            std::thread::sleep(std::time::Duration::from_millis(100));//to avoid write race
                            let content = match fs::read_to_string(&path_clone) {
                                Ok(c) => c,
                                Err(e) => { eprintln!("Failed to read config: {}", e); return; }
                            };
                            // Graceful parse — bad YAML just skips the update
                            let config: Config = match serde_yaml::from_str(&content) {
                                Ok(c) => c,
                                Err(e) => { eprintln!("Failed to parse config: {}", e); return; }
                            };
                            let _ = sender_clone.blocking_send(config.backends);
                        }
                    },
                    Err(e) => eprintln!("Error watching file: {:?}", e),
                }
            }).expect("Failed to create watcher");

            watcher.watch(Path::new(&path), notify::RecursiveMode::NonRecursive).expect("Failed to watch file");

            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
    }
}