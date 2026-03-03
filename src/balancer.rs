use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use reqwest::Client;

pub type SharedState = Arc<GateWayState>;
//A struct to hold the state of the gateway
pub struct GateWayState {
    pub backends: Vec<String>,
    pub counter: AtomicUsize,//to avoid data race
    pub client: Client,//to have a single client at start up for all connections 
}

impl GateWayState {
    pub fn next_backend(&self) -> &str {
        //round robin
        //chore: will make this better with better alorthims 
        let index = self.counter.fetch_add(1, Ordering::Relaxed) % self.backends.len();//atomic adding and return the index
        &self.backends[index]
    }

}



