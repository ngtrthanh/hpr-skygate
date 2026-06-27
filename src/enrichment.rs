/// Self-learning enrichment: routes learned from live data + traffic-api oracle

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};

const CACHE_FILE: &str = "/var/lib/fa3/learned_routes.json";
const BATCH_INTERVAL: Duration = Duration::from_secs(5);
const PERSIST_INTERVAL: Duration = Duration::from_secs(60);
const RETRY_AFTER: Duration = Duration::from_secs(86400); // 24h

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct LearnedRoute {
    pub airports: Option<String>,  // "VTBS-RJTT"
    pub airline: Option<String>,   // "Thai Airways"
    #[serde(default)]
    pub first_seen: u64,
    #[serde(default)]
    pub hits: u32,
}

pub struct Enrichment {
    pub airlines: HashMap<String, String>,  // ICAO prefix → airline name (6k, loaded at start)
    routes: HashMap<String, LearnedRoute>,
    pending: Vec<String>,
    last_batch: Instant,
    last_persist: Instant,
    traffic_api: String,
    failed: HashMap<String, Instant>,  // callsigns that failed lookup, retry after 24h
}

impl Enrichment {
    pub fn new(traffic_api_url: &str) -> Self {
        let routes = load_cache();
        tracing::info!(cached = routes.len(), "enrichment: loaded learned routes");
        Self {
            airlines: HashMap::new(),
            routes,
            pending: Vec::new(),
            last_batch: Instant::now(),
            last_persist: Instant::now(),
            traffic_api: traffic_api_url.to_string(),
            failed: HashMap::new(),
        }
    }

    /// Fast hot-path lookup. Returns route airports if known.
    pub fn get_route(&mut self, callsign: &str) -> Option<&str> {
        if let Some(r) = self.routes.get_mut(callsign) {
            r.hits += 1;
            return r.airports.as_deref();
        }
        // Schedule for background lookup
        if !self.pending.contains(&callsign.to_string()) {
            if let Some(t) = self.failed.get(callsign) {
                if t.elapsed() < RETRY_AFTER { return None; }
            }
            self.pending.push(callsign.to_string());
        }
        None
    }

    /// Get airline name from callsign prefix (first 3 chars)
    pub fn get_airline(&self, callsign: &str) -> Option<&str> {
        if callsign.len() >= 3 {
            self.airlines.get(&callsign[..3]).map(|s| s.as_str())
        } else {
            None
        }
    }

    /// Call periodically from the main loop (every flush tick)
    pub fn tick(&mut self) {
        let now = Instant::now();

        // Batch lookup pending callsigns
        if !self.pending.is_empty() && now.duration_since(self.last_batch) >= BATCH_INTERVAL {
            let n = self.pending.len().min(20);
            let batch: Vec<String> = self.pending.drain(..n).collect();
            for cs in &batch {
                match lookup_route(&self.traffic_api, cs) {
                    Some(route) => { self.routes.insert(cs.clone(), route); }
                    None => { self.failed.insert(cs.clone(), now); }
                }
            }
            self.last_batch = now;
        }

        // Persist to disk
        if now.duration_since(self.last_persist) >= PERSIST_INTERVAL {
            persist_cache(&self.routes);
            self.last_persist = now;
        }
    }

    pub fn route_count(&self) -> usize { self.routes.len() }
}

fn lookup_route(api_url: &str, callsign: &str) -> Option<LearnedRoute> {
    let url = format!("{}/v1/routes/{}", api_url, callsign);
    let body = http_get(&url)?;

    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let airports = v.get("airport_codes").and_then(|v| v.as_str()).map(|s| s.to_string());
    let airline_code = v.get("airline_code").and_then(|v| v.as_str()).unwrap_or("");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

    Some(LearnedRoute {
        airports,
        airline: if airline_code.is_empty() { None } else { Some(airline_code.to_string()) },
        first_seen: now,
        hits: 0,
    })
}

fn http_get(url: &str) -> Option<String> {
    let url_parts: Vec<&str> = url.strip_prefix("http://")?.splitn(2, '/').collect();
    let host = url_parts[0];
    let path = format!("/{}", url_parts.get(1).unwrap_or(&""));

    use std::net::ToSocketAddrs;
    let addr = host.to_socket_addrs().ok()?.next()?;
    let mut stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(1000))).ok()?;

    use std::io::Write;
    write!(stream, "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", path, host).ok()?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok();
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1)?;
    Some(body.to_string())
}

fn load_cache() -> HashMap<String, LearnedRoute> {
    match std::fs::read_to_string(CACHE_FILE) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

fn persist_cache(routes: &HashMap<String, LearnedRoute>) {
    let dir = std::path::Path::new(CACHE_FILE).parent().unwrap();
    std::fs::create_dir_all(dir).ok();
    if let Ok(data) = serde_json::to_string(routes) {
        std::fs::write(CACHE_FILE, data).ok();
    }
}
