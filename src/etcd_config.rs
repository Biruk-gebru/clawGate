use tokio::sync::mpsc;
use etcd_client::{Client, EventType, WatchOptions, WatchStream};

use crate::config::BackendConfig;

/// Connects to etcd, reads the initial backend config from the given key,
/// then watches for changes and sends updated backend lists through the channel.
///
/// The value stored in etcd should be a YAML array of BackendConfig entries:
/// ```yaml
/// - url: "http://127.0.0.1:4000"
///   weight: 3
/// - url: "http://127.0.0.1:4001"
/// ```
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
