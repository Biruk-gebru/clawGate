use std::collections::VecDeque;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use dashmap::DashMap;

/// Sliding window of request timestamps for a single IP.
pub struct RateBucket {
    pub timestamps: VecDeque<Instant>,
}

impl RateBucket {
    pub fn new() -> Self {
        RateBucket { timestamps: VecDeque::new() }
    }

    /// Returns true if under the limit (and records the request), false otherwise.
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

/// Concurrent per-IP rate limiter backed by DashMap.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_allows_under_limit() {
        let mut bucket = RateBucket::new();
        assert!(bucket.is_allowed(3, 60));
        assert!(bucket.is_allowed(3, 60));
        assert!(bucket.is_allowed(3, 60));
    }

    #[test]
    fn bucket_rejects_over_limit() {
        let mut bucket = RateBucket::new();
        assert!(bucket.is_allowed(2, 60));
        assert!(bucket.is_allowed(2, 60));
        assert!(!bucket.is_allowed(2, 60));
    }

    #[test]
    fn limiter_tracks_per_ip() {
        let limiter = RateLimiter::new(1, 60);
        let ip1: IpAddr = "10.0.0.1".parse().unwrap();
        let ip2: IpAddr = "10.0.0.2".parse().unwrap();

        assert!(limiter.check_and_record(ip1));
        assert!(!limiter.check_and_record(ip1)); // ip1 exhausted
        assert!(limiter.check_and_record(ip2));  // ip2 still has quota
    }

    #[test]
    fn evict_removes_stale_entries() {
        let limiter = RateLimiter::new(100, 0); // 0-second window = everything is stale
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        limiter.check_and_record(ip);
        std::thread::sleep(std::time::Duration::from_millis(10));
        limiter.evict_stale();
        assert_eq!(limiter.map.len(), 0);
    }
}