use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Instant;

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
        std::thread::spawn(move || handle(stream, &tracker, &jc, &bc, &tc, wd.as_deref(), decode_enabled));
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
) {
    let mut buf = [0u8; 2048];
    let n = stream.read(&mut buf).unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req.split_whitespace().nth(1).unwrap_or("/");
    let path = path.split('?').next().unwrap_or(path);
    let query = req.split_whitespace().nth(1).unwrap_or("").split('?').nth(1).unwrap_or("");

    // re-api (tar1090 binCraft with bbox)
    if path.starts_with("/re-api") {
        if decode_enabled {
            let data = bincraft_cache.read().unwrap().clone();
            // TODO: bbox filter on binary data if query has &box=
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
    if path.starts_with("/api/aircraft") {
        if decode_enabled { send(&mut stream, "200 OK", "application/json", &json_cache.read().unwrap()); }
        else { send(&mut stream, "503 Unavailable", "text/plain", b"decode off"); }
        return;
    }
    if path == "/health" {
        let j = serde_json::json!({ "status": "ok", "feeders": tracker.count(),
            "uptime_seconds": START.get().map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0) });
        send(&mut stream, "200 OK", "application/json", j.to_string().as_bytes());
        return;
    }
    if path == "/api/feeders" {
        let mut list = tracker.snapshot();
        list.sort_by(|a, b| b.bytes_recv.cmp(&a.bytes_recv));
        let j = serde_json::json!({ "total": list.len(), "feeders": list });
        send(&mut stream, "200 OK", "application/json", j.to_string().as_bytes());
        return;
    }
    if path == "/data/receiver.json" {
        // Try static file first, fall through to generated
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
