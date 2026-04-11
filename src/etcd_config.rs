use tokio::sync::mpsc;
use etcd_client::{Client, EventType, WatchOptions, WatchStream};

use crate::config::BackendConfig;

/// Connects to etcd, loads the initial backend config, then watches for changes.
/// Updated backend lists are sent through the same channel as the config file watcher.
/// Falls back gracefully if etcd is unreachable.
pub async fn start_etcd_watcher(endpoint: &str, key: &str, sender: mpsc::Sender<Vec<BackendConfig>>) {
    let mut client = match Client::connect([endpoint], None).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("etcd: failed to connect ({}), falling back to local config", e);
            return;
        }
    };

    // Load the initial value
    let resp = match client.get(key.as_bytes(), None).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("etcd: failed to read key ({}), falling back to local config", e);
            return;
        }
    };
    if let Some(kv) = resp.kvs().first() {
        if let Ok(backends) = serde_yaml::from_slice::<Vec<BackendConfig>>(kv.value()) {
            let _ = sender.send(backends).await;
        }
    }

    // Watch for changes
    let mut stream: WatchStream = client
        .watch(key.as_bytes(), Some(WatchOptions::new()))
        .await
        .expect("Failed to start etcd watch");

    while let Some(resp) = stream.message().await.expect("etcd watch stream error") {
        for event in resp.events() {
            if event.event_type() == EventType::Put {
                if let Some(kv) = event.kv() {
                    if let Ok(backends) = serde_yaml::from_slice::<Vec<BackendConfig>>(kv.value()) {
                        let _ = sender.send(backends).await;
                    }
                }
            }
        }
    }
}
