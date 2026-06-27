use std::collections::HashSet;
use std::sync::RwLock;
use std::time::Instant;

use dashmap::DashMap;
use serde::Serialize;

use crate::client::ClientState;

#[derive(Debug, Clone, Serialize)]
pub struct FeederInfo {
    pub addr: String,
    pub uuid: Option<String>,
    pub receiver_id: String,
    pub bytes_recv: u64,
    pub msg_count: u64,
    pub uptime_sec: f64,
    pub garbage: bool,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

struct Entry {
    uuid: Option<String>,
    receiver_id: u64,
    bytes_recv: u64,
    msg_count: u64,
    connected_at: Instant,
    garbage: bool,
    lat: Option<f64>,
    lon: Option<f64>,
    last_msg_at: Instant,
}

pub struct FeederTracker {
    feeders: DashMap<String, Entry>,
    blocked: RwLock<HashSet<String>>,
}

impl FeederTracker {
    pub fn new() -> Self {
        Self {
            feeders: DashMap::new(),
            blocked: RwLock::new(HashSet::new()),
        }
    }

    pub fn connect(&self, addr: &str) {
        let now = Instant::now();
        self.feeders.insert(addr.to_string(), Entry {
            uuid: None,
            receiver_id: 0,
            bytes_recv: 0,
            msg_count: 0,
            connected_at: now,
            garbage: false,
            lat: None,
            lon: None,
            last_msg_at: now,
        });
    }

    pub fn update(&self, addr: &str, client: &ClientState) {
        if let Some(mut e) = self.feeders.get_mut(addr) {
            e.uuid.clone_from(&client.uuid);
            e.receiver_id = client.receiver_id;
            e.bytes_recv = client.bytes_recv;
            e.msg_count = client.msg_count;
            e.garbage = client.garbage;
            e.last_msg_at = Instant::now();
        }
    }

    pub fn disconnect(&self, addr: &str) {
        self.feeders.remove(addr);
    }

    pub fn is_blocked(&self, addr: &str) -> bool {
        let host = addr.split(':').next().unwrap_or(addr);
        let blocked = self.blocked.read().unwrap();
        blocked.contains(host) || blocked.contains(addr)
    }

    pub fn block(&self, id: &str) {
        self.blocked.write().unwrap().insert(id.to_string());
        self.feeders.retain(|k, e| {
            let host = k.split(':').next().unwrap_or(k);
            !(host == id || k.as_str() == id || e.uuid.as_deref() == Some(id))
        });
    }

    pub fn unblock(&self, id: &str) {
        self.blocked.write().unwrap().remove(id);
    }

    pub fn count(&self) -> usize {
        self.feeders.len()
    }

    pub fn set_location(&self, uuid_or_addr: &str, lat: f64, lon: f64) {
        for mut e in self.feeders.iter_mut() {
            if e.key() == uuid_or_addr || e.value().uuid.as_deref() == Some(uuid_or_addr) {
                e.lat = Some(lat);
                e.lon = Some(lon);
                return;
            }
        }
    }

    pub fn stalled_feeders(&self, timeout_secs: u64) -> Vec<String> {
        let now = Instant::now();
        let mut stalled = Vec::new();
        for e in self.feeders.iter() {
            if now.duration_since(e.last_msg_at).as_secs() > timeout_secs {
                stalled.push(e.key().clone());
            }
        }
        stalled
    }

    pub fn snapshot(&self) -> Vec<FeederInfo> {
        let now = Instant::now();
        self.feeders.iter().map(|e| {
            let v = e.value();
            FeederInfo {
                addr: e.key().clone(),
                uuid: v.uuid.clone(),
                receiver_id: format!("{:016x}", v.receiver_id),
                bytes_recv: v.bytes_recv,
                msg_count: v.msg_count,
                uptime_sec: now.duration_since(v.connected_at).as_secs_f64(),
                garbage: v.garbage,
                lat: v.lat,
                lon: v.lon,
            }
        }).collect()
    }
}
