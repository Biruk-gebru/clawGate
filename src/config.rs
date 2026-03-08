use std::fs;
use tokio::sync::mpsc;
use notify::Watcher;
use std::path::Path;

#[derive(serde::Deserialize)]
pub struct Config {
    pub backends: Vec<String>
}

impl Config {
    pub fn load_config() -> Config {
        let path = "config.yaml";
        let content = fs::read_to_string(path).expect("Failed to read config");
        
        serde_yaml::from_str(&content).expect("Failed to parse config")
    }

    pub fn start_watcher(path: &str, sender: mpsc::Sender<Vec<String>>) {
        let path = path.to_string();

        std::thread::spawn(move || {
            let sender_clone = sender.clone();

            let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>|{
                match result {
                    Ok(event) => {
                        if let notify::EventKind::Modify(_) = event.kind {
                            let content = read_to_string(&path).expect("Failed to read the config file");
                            let config: Config = serde_yaml::from_str(&content).expect("Failed to parse config");
                            let _ = sender_clone.blocking_send(config.backends);
                        }
                    },
                    Err(e) => eprintln!("Error watching file: {:?}", e),
                }
            }).expect("Failed to create watcher");
            
            watcher.watch(Path::new(&path), notify::RecursiveMode::NonRecursive).expect("Failed to watch file");

            loop {
                std::thread::sleep(std::time:;Duration::from_Secs(1));
            }
            

        })
        
    }
}