use crate::dashboard::{CircuitState, SharedDashboard};
use reqwest::Client;
use std::time::{Duration, Instant};
use tokio::time;

pub fn start_health_checker(
    dashboard: SharedDashboard,
    client: Client,
    interval_secs: u64,
    cooldown_secs: u64,
    failure_threshold: u64,
) {
    tokio::spawn(async move {
        let mut ticker = time::interval(Duration::from_secs(interval_secs));

        loop {
            ticker.tick().await;

            // Snapshot URLs without holding the lock during network I/O
            let backends_snapshot: Vec<(String, String)> = {
                let dash = dashboard.lock().unwrap();
                dash.backends
                    .iter()
                    .map(|b| (b.url.clone(), b.health_path.clone()))
                    .collect()
            };

            for (url, health_path) in backends_snapshot {
                let target = format!("{}{}", url, health_path);

                let is_healthy = client
                    .get(&target)
                    .timeout(Duration::from_secs(2))
                    .send()
                    .await
                    .map(|r| r.status().as_u16() < 500)
                    .unwrap_or(false);

                let mut dash = dashboard.lock().unwrap();
                if let Some(info) = dash.backends.iter_mut().find(|b| b.url == url) {
                    info.last_checked = Some(Instant::now());

                    if is_healthy {
                        let was_open = info.circuit_state != CircuitState::Closed;
                        info.failed_count = 0;
                        info.is_healthy = true;
                        info.circuit_state = CircuitState::Closed;
                        if was_open {
                            dash.status_msg = format!("✓ {} recovered -circuit Closed", url);
                        }
                    } else {
                        info.is_healthy = false;
                        info.failed_count += 1;

                        match info.circuit_state {
                            CircuitState::Closed => {
                                if info.failed_count >= failure_threshold {
                                    info.circuit_state = CircuitState::Open {
                                        tripped_at: Instant::now(),
                                    };
                                    dash.status_msg = format!(
                                        "⚡ {} tripped -circuit Open ({}s cooldown)", url, cooldown_secs
                                    );
                                } else {
                                    dash.status_msg = format!(
                                        "⚠ {} unhealthy ({}/{} failures)", url, info.failed_count, failure_threshold
                                    );
                                }
                            }
                            CircuitState::Open { tripped_at } => {
                                if tripped_at.elapsed() >= Duration::from_secs(cooldown_secs) {
                                    info.circuit_state = CircuitState::HalfOpen;
                                    dash.status_msg = format!(
                                        "⏳ {} cooldown elapsed -entering HalfOpen", url
                                    );
                                }
                            }
                            CircuitState::HalfOpen => {
                                info.failed_count = 1;
                                info.circuit_state = CircuitState::Open {
                                    tripped_at: Instant::now(),
                                };
                                dash.status_msg = format!("⚡ {} probe failed -circuit re-Opened", url);
                            }
                        }
                    }
                }
            }
        }
    });
}