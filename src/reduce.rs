use std::collections::HashMap;
use std::time::Instant;

pub struct ReduceState {
    last_sent: HashMap<u32, Instant>,
    interval: std::time::Duration,
}

impl ReduceState {
    pub fn new(interval_secs: f64) -> Self {
        Self { last_sent: HashMap::new(), interval: std::time::Duration::from_secs_f64(interval_secs) }
    }
    pub fn should_emit(&mut self, icao: u32, now: Instant) -> bool {
        match self.last_sent.get(&icao) {
            Some(last) if now.duration_since(*last) < self.interval => false,
            _ => { self.last_sent.insert(icao, now); true }
        }
    }
}
