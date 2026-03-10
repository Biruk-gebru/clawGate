use crate::dashboard::SharedDashboard;
use reqwest::Client;
use std::time::{Duration, Instant};
use tokio::time;

pub fn start_health_checker(dashboard: SharedDashboard, client: Client, interval_secs: u64) {
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
                    let was_healthy = info.is_healthy;
                    info.is_healthy = is_healthy;
                    info.last_checked = Some(Instant::now());

                    // Notify via TUI status message when a backend changes state
                    if was_healthy && !is_healthy {
                        dash.status_msg = format!("⚠ {} went DOWN", url);
                    } else if !was_healthy && is_healthy {
                        dash.status_msg = format!("✓ {} is back UP", url);
                    }
                }
            }
        }
    });
}