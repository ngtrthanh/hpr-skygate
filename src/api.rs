use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use crate::ais;
use crate::alerts::AlertStore;
use crate::feeder::FeederTracker;

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

pub fn run_blocking(
    tracker: Arc<FeederTracker>,
    addr: &str,
    json_cache: Arc<RwLock<Vec<u8>>>,
    bincraft_cache: Arc<RwLock<Vec<u8>>>,
    trace_cache: Arc<RwLock<std::collections::HashMap<u32, String>>>,
    web_dir: Option<String>,
    decode_enabled: bool,
    alert_store: Arc<RwLock<AlertStore>>,
    kick_list: Arc<RwLock<Vec<String>>>,
    rate_overrides: Arc<RwLock<HashMap<String, u64>>>,
    vessel_store: Arc<RwLock<ais::vessel::VesselStore>>,
) {
    START.get_or_init(Instant::now);
    let listener = TcpListener::bind(addr).expect("bind http");
    tracing::info!(addr, "http listening");
    let web_dir = web_dir.map(PathBuf::from);

    for stream in listener.incoming().flatten() {
        let tracker = Arc::clone(&tracker);
        let jc = Arc::clone(&json_cache);
        let bc = Arc::clone(&bincraft_cache);
        let tc = Arc::clone(&trace_cache);
        let wd = web_dir.clone();
        let als = Arc::clone(&alert_store);
        let kl = Arc::clone(&kick_list);
        let ro = Arc::clone(&rate_overrides);
        let vs = Arc::clone(&vessel_store);
        std::thread::spawn(move || handle(stream, &tracker, &jc, &bc, &tc, wd.as_deref(), decode_enabled, &als, &kl, &ro, &vs));
    }
}

fn handle(
    mut stream: std::net::TcpStream,
    tracker: &FeederTracker,
    json_cache: &Arc<RwLock<Vec<u8>>>,
    bincraft_cache: &Arc<RwLock<Vec<u8>>>,
    trace_cache: &Arc<RwLock<std::collections::HashMap<u32, String>>>,
    web_dir: Option<&Path>,
    decode_enabled: bool,
    alert_store: &Arc<RwLock<AlertStore>>,
    kick_list: &Arc<RwLock<Vec<String>>>,
    rate_overrides: &Arc<RwLock<HashMap<String, u64>>>,
    vessel_store: &Arc<RwLock<ais::vessel::VesselStore>>,
) {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    if n == 0 { return; }
    let req = String::from_utf8_lossy(&buf[..n]);
    let first_line = req.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let raw_path = parts.next().unwrap_or("/");
    let is_post = method == "POST";
    let path = raw_path.split('?').next().unwrap_or(raw_path);
    let query = raw_path.split('?').nth(1).unwrap_or("");

    // Extract body for POST requests
    let req_str = req.to_string();
    let body = req_str.split("\r\n\r\n").nth(1).unwrap_or("");

    // re-api (tar1090 binCraft with bbox)
    if path.starts_with("/re-api") {
        if decode_enabled {
            let data = bincraft_cache.read().unwrap().clone();
            send(&mut stream, "200 OK", "application/octet-stream", &data);
        } else {
            send(&mut stream, "503 Unavailable", "text/plain", b"decode off");
        }
        return;
    }

    if path == "/api/aircraft.binCraft" {
        if decode_enabled { send(&mut stream, "200 OK", "application/octet-stream", &bincraft_cache.read().unwrap()); }
        else { send(&mut stream, "503 Unavailable", "text/plain", b"decode off"); }
        return;
    }
    if path.starts_with("/api/trace/") {
        if decode_enabled {
            let hex_str = path.strip_prefix("/api/trace/").unwrap_or("");
            if let Ok(icao) = u32::from_str_radix(hex_str, 16) {
                let tc = trace_cache.read().unwrap();
                if let Some(trace_json) = tc.get(&icao) {
                    send(&mut stream, "200 OK", "application/json", trace_json.as_bytes());
                } else {
                    send(&mut stream, "404 Not Found", "application/json", b"[]");
                }
            } else {
                send(&mut stream, "400 Bad Request", "text/plain", b"invalid hex");
            }
        } else {
            send(&mut stream, "503 Unavailable", "text/plain", b"decode off");
        }
        return;
    }
    if path == "/api/aircraft" {
        if decode_enabled { send(&mut stream, "200 OK", "application/json", &json_cache.read().unwrap()); }
        else { send(&mut stream, "503 Unavailable", "text/plain", b"decode off"); }
        return;
    }
    if path == "/api/vessels" {
        let json = vessel_store.read().unwrap().to_json();
        send(&mut stream, "200 OK", "application/json", &json);
        return;
    }
    if path == "/health" {
        let j = serde_json::json!({ "status": "ok", "feeders": tracker.count(),
            "vessels": vessel_store.read().unwrap().count(),
            "uptime_seconds": START.get().map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0) });
        send(&mut stream, "200 OK", "application/json", j.to_string().as_bytes());
        return;
    }

    // M3: Alerts
    if path == "/api/alerts" {
        let store = alert_store.read().unwrap();
        let limit = query_param(query, "limit").and_then(|s| s.parse().ok()).unwrap_or(50usize);
        let alerts: Vec<_> = store.recent(limit).iter().map(|a| serde_json::to_value(a).unwrap()).collect();
        send(&mut stream, "200 OK", "application/json", serde_json::to_string(&alerts).unwrap().as_bytes());
        return;
    }

    // M2: Feeder map
    if path == "/api/feeders/map" {
        let list = tracker.snapshot();
        let features: Vec<_> = list.iter().filter_map(|f| {
            let (lat, lon) = (f.lat?, f.lon?);
            Some(serde_json::json!({
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [lon, lat] },
                "properties": { "uuid": f.uuid, "addr": f.addr, "msg_count": f.msg_count }
            }))
        }).collect();
        let geojson = serde_json::json!({ "type": "FeatureCollection", "features": features });
        send(&mut stream, "200 OK", "application/json", geojson.to_string().as_bytes());
        return;
    }

    // M4: Feeder stats summary
    if path == "/api/feeders/stats" {
        let mut list = tracker.snapshot();
        let active = list.iter().filter(|f| !f.garbage).count();
        let garbage = list.iter().filter(|f| f.garbage).count();
        let total_msgs: u64 = list.iter().map(|f| f.msg_count).sum();
        list.sort_by(|a, b| b.msg_count.cmp(&a.msg_count));
        let top: Vec<_> = list.iter().take(10).map(|f| serde_json::json!({"addr": f.addr, "uuid": f.uuid, "msgs": f.msg_count})).collect();
        let j = serde_json::json!({"total": list.len(), "active": active, "garbage": garbage, "total_messages": total_msgs, "top_feeders": top});
        send(&mut stream, "200 OK", "application/json", j.to_string().as_bytes());
        return;
    }

    // M4: Kick
    if path == "/api/feeders/kick" && is_post {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(id) = v.get("id").and_then(|v| v.as_str()) {
                kick_list.write().unwrap().push(id.to_string());
                send(&mut stream, "200 OK", "application/json", b"{\"kicked\":true}");
                return;
            }
        }
        send(&mut stream, "400 Bad Request", "application/json", b"{\"error\":\"need id\"}");
        return;
    }

    // M4: Rate limit override
    if path == "/api/feeders/rate-limit" && is_post {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            let id = v.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let limit = v.get("limit").and_then(|v| v.as_u64()).unwrap_or(5420);
            rate_overrides.write().unwrap().insert(id.to_string(), limit);
            send(&mut stream, "200 OK", "application/json", b"{\"ok\":true}");
        } else {
            send(&mut stream, "400 Bad Request", "application/json", b"{\"error\":\"bad json\"}");
        }
        return;
    }

    // Existing block/unblock
    if path == "/api/feeders/block" && is_post {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(id) = v.get("id").and_then(|v| v.as_str()) {
                tracker.block(id);
                send(&mut stream, "200 OK", "application/json", b"{\"blocked\":true}");
                return;
            }
        }
        send(&mut stream, "400 Bad Request", "application/json", b"{\"error\":\"need id\"}");
        return;
    }
    if path == "/api/feeders/unblock" && is_post {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(id) = v.get("id").and_then(|v| v.as_str()) {
                tracker.unblock(id);
                send(&mut stream, "200 OK", "application/json", b"{\"unblocked\":true}");
                return;
            }
        }
        send(&mut stream, "400 Bad Request", "application/json", b"{\"error\":\"need id\"}");
        return;
    }

    // M2: Set feeder location (must be before single feeder lookup)
    if path.starts_with("/api/feeders/") && path.ends_with("/location") && is_post {
        let uuid_str = path.strip_prefix("/api/feeders/").unwrap().strip_suffix("/location").unwrap_or("");
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            let lat = v.get("lat").and_then(|v| v.as_f64());
            let lon = v.get("lon").and_then(|v| v.as_f64());
            if let (Some(lat), Some(lon)) = (lat, lon) {
                tracker.set_location(uuid_str, lat, lon);
                send(&mut stream, "200 OK", "application/json", b"{\"ok\":true}");
            } else {
                send(&mut stream, "400 Bad Request", "application/json", b"{\"error\":\"need lat,lon\"}");
            }
        } else {
            send(&mut stream, "400 Bad Request", "application/json", b"{\"error\":\"bad json\"}");
        }
        return;
    }

    // M4: List feeders (existing)
    if path == "/api/feeders" {
        let mut list = tracker.snapshot();
        list.sort_by(|a, b| b.bytes_recv.cmp(&a.bytes_recv));
        let j = serde_json::json!({ "total": list.len(), "feeders": list });
        send(&mut stream, "200 OK", "application/json", j.to_string().as_bytes());
        return;
    }

    // M4: Single feeder by UUID
    if path.starts_with("/api/feeders/") {
        let uuid_str = path.strip_prefix("/api/feeders/").unwrap_or("");
        if !uuid_str.is_empty() {
            let list = tracker.snapshot();
            if let Some(f) = list.iter().find(|f| f.uuid.as_deref() == Some(uuid_str) || f.addr == uuid_str) {
                send(&mut stream, "200 OK", "application/json", serde_json::to_string(f).unwrap().as_bytes());
            } else {
                send(&mut stream, "404 Not Found", "application/json", b"{\"error\":\"not found\"}");
            }
            return;
        }
    }

    if path == "/data/receiver.json" {
        let j = r#"{"refresh":1000,"history":0,"readsb":true,"dbServer":true,"binCraft":true}"#;
        send(&mut stream, "200 OK", "application/json", j.as_bytes());
        return;
    }
    if path == "/data/aircraft.binCraft" {
        if decode_enabled { send(&mut stream, "200 OK", "application/octet-stream", &bincraft_cache.read().unwrap()); }
        else { send(&mut stream, "503 Unavailable", "text/plain", b"decode off"); }
        return;
    }
    if path == "/data/aircraft.json" {
        if decode_enabled { send(&mut stream, "200 OK", "application/json", &json_cache.read().unwrap()); }
        else { send(&mut stream, "503 Unavailable", "text/plain", b"decode off"); }
        return;
    }

    // Static files
    if let Some(dir) = web_dir {
        let file_path = if path == "/" { "index.html" } else { path.trim_start_matches('/') };
        let file_path = file_path.replace("..", "");
        let full = dir.join(&file_path);
        if full.is_file() {
            if let Ok(data) = fs::read(&full) {
                send(&mut stream, "200 OK", content_type(&file_path), &data);
                return;
            }
        }
        send(&mut stream, "404 Not Found", "text/plain", b"not found");
        return;
    }

    send(&mut stream, "200 OK", "text/html", include_str!("../dashboard.html").as_bytes());
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let mut kv = pair.splitn(2, '=');
        if kv.next()? == key { kv.next() } else { None }
    })
}

fn content_type(p: &str) -> &'static str {
    if p.ends_with(".js") { "application/javascript" }
    else if p.ends_with(".css") { "text/css" }
    else if p.ends_with(".html") { "text/html" }
    else if p.ends_with(".json") { "application/json" }
    else if p.ends_with(".png") { "image/png" }
    else if p.ends_with(".svg") { "image/svg+xml" }
    else { "application/octet-stream" }
}

fn send(stream: &mut std::net::TcpStream, status: &str, ct: &str, body: &[u8]) {
    let h = format!("HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n", status, ct, body.len());
    stream.write_all(h.as_bytes()).ok();
    stream.write_all(body).ok();
}
