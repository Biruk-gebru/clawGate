use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use chitchat::{ChitchatConfig, ChitchatId, FailureDetectorConfig, spawn_chitchat, transport::UdpTransport};
use crate::dashboard::SharedDashboard;

/// Sanitise a backend URL into a key-safe string.
/// "http://127.0.0.1:4000" becomes "http_127.0.0.1-4000"
fn url_to_key(url: &str) -> String {
    url.replace("://", "_").replace(':', "-")
}

pub async fn start_gossip(
    node_id: String,
    listen_addr: SocketAddr,
    seed_nodes: Vec<String>,
    dashboard: SharedDashboard,
) {
    // ChitchatId uniquely identifies this node in the cluster.
    // The generation number distinguishes restarts of the same node
    // so peers don't confuse stale state from a previous run.
    let chitchat_id = ChitchatId::new(node_id, 0, listen_addr);

    let config = ChitchatConfig {
        chitchat_id,
        cluster_id: "clawgate".to_string(),
        gossip_interval: Duration::from_secs(1),
        listen_addr,
        seed_nodes,
        failure_detector_config: FailureDetectorConfig::default(),
        marked_for_deletion_grace_period: Duration::from_secs(30),
        catchup_callback: None,
        extra_liveness_predicate: None,
    };

    // spawn_chitchat starts the UDP gossip listener in the background
    // and returns a handle we use to read/write shared state.
    let handle = match spawn_chitchat(config, Vec::new(), &UdpTransport).await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("gossip: failed to start ({}), running without peer sync", e);
            return;
        }
    };

    let handle = Arc::new(handle);
    let dashboard_writer = dashboard.clone();
    let handle_writer = Arc::clone(&handle);

    // WRITER TASK: every 2 seconds, publish our local health state
    // into chitchat so peers can see it.
    // We snapshot the dashboard under a short lock, then write to
    // chitchat after releasing it (avoids holding both locks).
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;

            let snapshot: Vec<(String, bool, String)> = {
                let dash = dashboard_writer.lock().unwrap();
                dash.backends.iter().map(|b| {
                    (b.url.clone(), b.is_healthy, b.circuit_state.to_db_string().to_string())
                }).collect()
            };

            let chitchat_mutex = handle_writer.chitchat();
            let mut chitchat = chitchat_mutex.lock().await;
            for (url, healthy, circuit) in &snapshot {
                let key_prefix = url_to_key(url);
                chitchat.self_node_state().set(
                    format!("backend/{}/is_healthy", key_prefix),
                    if *healthy { "true" } else { "false" },
                );
                chitchat.self_node_state().set(
                    format!("backend/{}/circuit_state", key_prefix),
                    circuit.as_str(),
                );
            }
        }
    });

    // READER TASK: every 2 seconds, read peer states from chitchat.
    // If any peer reports a backend as unhealthy that we think is healthy,
    // mark it unhealthy locally (pessimistic merge).
    // Snapshot peer data first, then lock dashboard to apply updates.
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;

            // Snapshot all peer state while holding the chitchat lock
            let mut peer_reports: Vec<(String, bool)> = Vec::new();
            {
                let chitchat_mutex = handle.chitchat();
                let chitchat = chitchat_mutex.lock().await;
                for (node_id, node_state) in chitchat.node_states() {
                    if *node_id == *chitchat.self_chitchat_id() {
                        continue;
                    }
                    for (key, value) in node_state.key_values() {
                        let key: &str = key;
                        if let Some(url_key) = key.strip_prefix("backend/") {
                            if let Some(url_key) = url_key.strip_suffix("/is_healthy") {
                                if value == "false" {
                                    peer_reports.push((url_key.to_string(), false));
                                }
                            }
                        }
                    }
                }
            }
            // chitchat lock released here

            if peer_reports.is_empty() {
                continue;
            }

            // Apply peer reports to our dashboard
            let mut dash = dashboard.lock().unwrap();
            for (url_key, _) in &peer_reports {
                if let Some(info) = dash.backends.iter_mut().find(|b| url_to_key(&b.url) == *url_key) {
                    if info.is_healthy {
                        info.is_healthy = false;
                        dash.status_msg = format!("Peer reports {} unhealthy", info.url);
                    }
                }
            }
        }
    });
}
