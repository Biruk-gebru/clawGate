use std::collections::VecDeque;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use dashmap::DashMap;

pub struct RateBucket {
    pub timestamps: VecDeque<Instant>,
}

impl RateBucket {
    pub fn new() -> Self {
        RateBucket { timestamps: VecDeque::new() }
    }

    /// Returns true and records the request if under the limit, false if at/over it.
    /// Prunes timestamps older than `window_secs` before checking.
    pub fn is_allowed(&mut self, limit: u64, window_secs: u64) -> bool {
        let now = Instant::now();
        let window = Duration::from_secs(window_secs);
        while let Some(&front) = self.timestamps.front() {
            if now.duration_since(front) > window { self.timestamps.pop_front(); } else { break; }
        }
        if self.timestamps.len() < limit as usize {
            self.timestamps.push_back(now);
            true
        } else {
            false
        }
    }

    pub fn evict_stale(&mut self, window_secs: u64) {
        let now = Instant::now();
        let window = Duration::from_secs(window_secs);
        while let Some(&front) = self.timestamps.front() {
            if now.duration_since(front) > window { self.timestamps.pop_front(); } else { break; }
        }
    }
}

/// Concurrent per-IP rate limiter. DashMap shards internally so async tasks
/// rarely contend. Never hold a RefMut across an .await point.
pub struct RateLimiter {
    pub map: DashMap<IpAddr, RateBucket>,
    pub limit: u64,
    pub window_secs: u64,
}

impl RateLimiter {
    pub fn new(limit: u64, window_secs: u64) -> Self {
        RateLimiter { map: DashMap::new(), limit, window_secs }
    }

    pub fn check_and_record(&self, ip: IpAddr) -> bool {
        let mut bucket = self.map.entry(ip).or_insert_with(RateBucket::new);
        bucket.is_allowed(self.limit, self.window_secs)
    }

    pub fn evict_stale(&self) {
        self.map.retain(|_ip, bucket| {
            bucket.evict_stale(self.window_secs);
            !bucket.timestamps.is_empty()
        });
    }
}