use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use reqwest::Client;
use std::sync::RwLock;


use crate::dashboard::SharedDashboard;

pub type SharedState = Arc<GateWayState>;
//A struct to hold the state of the gateway
pub struct GateWayState {
    pub backends: Arc<RwLock<Vec<String>>>,
    pub counter: AtomicUsize,//to avoid data race
    pub client: Client,//to have a single client at start up for all connections 
    pub dashboard: SharedDashboard,//contain the logs
}

impl GateWayState {
    pub fn next_backend(&self) -> String {
        let backends = self.backends.read().unwrap();
        //round robin
        //chore: will make this better with better alorthims 
        let index = self.counter.fetch_add(1, Ordering::Relaxed) % backends.len();//atomic adding and return the index
        backends[index].clone()
    }

}



