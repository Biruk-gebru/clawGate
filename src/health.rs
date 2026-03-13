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

            // Snapshot backend urls/paths WITHOUT holding the lock during network I/O
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
                    .timeout(Duration::from_secs(2)) // 2 second probe timeout
                    .send()
                    .await
                    .map(|r| r.status().as_u16() < 500) // 2xx and 4xx = alive, 5xx = dead
                    .unwrap_or(false);               // connection refused / timeout = dead

                // Reacquire lock just to write the result
                let mut dash = dashboard.lock().unwrap();
                if let Some(info) = dash.backends.iter_mut().find(|b| b.url == url) {
                    info.last_checked = Some(Instant::now());

                    if is_healthy {
                        // Success: reset counter and close the circuit
                        let was_open = info.circuit_state != CircuitState::Closed;
                        info.failed_count = 0;
                        info.is_healthy = true;
                        info.circuit_state = CircuitState::Closed;
                        if was_open {
                            dash.status_msg = format!("✓ {} recovered — circuit Closed", url);
                        }
                    } else {
                        // Failure path: increment counter and drive the state machine
                        info.is_healthy = false;
                        info.failed_count += 1;

                        match info.circuit_state {
                            CircuitState::Closed => {
                                // Still counting failures — check if we should trip
                                if info.failed_count >= failure_threshold {
                                    info.circuit_state = CircuitState::Open {
                                        tripped_at: Instant::now(),
                                    };
                                    dash.status_msg = format!(
                                        "⚡ {} tripped — circuit Open ({}s cooldown)", url, cooldown_secs
                                    );
                                } else {
                                    dash.status_msg = format!(
                                        "⚠ {} unhealthy ({}/{} failures)", url, info.failed_count, failure_threshold
                                    );
                                }
                            }
                            CircuitState::Open { tripped_at } => {
                                // Check if cooldown has elapsed → move to HalfOpen
                                if tripped_at.elapsed() >= Duration::from_secs(cooldown_secs) {
                                    info.circuit_state = CircuitState::HalfOpen;
                                    dash.status_msg = format!(
                                        "⏳ {} cooldown elapsed — entering HalfOpen", url
                                    );
                                }
                                // else stay Open
                            }
                            CircuitState::HalfOpen => {
                                // Probe failed → trip again immediately
                                info.failed_count = 1;
                                info.circuit_state = CircuitState::Open {
                                    tripped_at: Instant::now(),
                                };
                                dash.status_msg = format!("⚡ {} probe failed — circuit re-Opened", url);
                            }
                        }
                    }
                }
            }
        }
    });
}