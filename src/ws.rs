use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub fn run_ws_server(addr: &str, cache: Arc<RwLock<Vec<u8>>>) {
    let listener = TcpListener::bind(addr).expect("bind ws port");
    tracing::info!(addr, "websocket listening");
    for stream in listener.incoming().flatten() {
        let c = Arc::clone(&cache);
        std::thread::spawn(move || handle(stream, c));
    }
}

fn handle(mut s: TcpStream, cache: Arc<RwLock<Vec<u8>>>) {
    let mut buf = [0u8; 2048];
    let n = match s.read(&mut buf) { Ok(n) if n > 0 => n, _ => return };
    let req = String::from_utf8_lossy(&buf[..n]);
    let key = match extract_key(&req) { Some(k) => k, None => return };
    let accept = ws_accept(&key);
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if s.write_all(resp.as_bytes()).is_err() { return; }
    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
    s.set_read_timeout(Some(Duration::from_millis(100))).ok();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let mut tmp = [0u8; 2];
        match s.read(&mut tmp) {
            Ok(0) => break,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
            _ => {}
        }
        let data = cache.read().unwrap().clone();
        if data.is_empty() { continue; }
        if ws_write_bin(&mut s, &data).is_err() { break; }
    }
}

fn ws_write_bin(s: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    let len = data.len();
    let mut hdr = Vec::with_capacity(10);
    hdr.push(0x82u8);
    if len < 126 {
        hdr.push(len as u8);
    } else if len < 65536 {
        hdr.push(126);
        hdr.push((len >> 8) as u8);
        hdr.push((len & 0xFF) as u8);
    } else {
        hdr.push(127);
        hdr.extend_from_slice(&(len as u64).to_be_bytes());
    }
    s.write_all(&hdr)?;
    s.write_all(data)
}

fn extract_key(req: &str) -> Option<String> {
    for line in req.lines() {
        if line.to_ascii_lowercase().starts_with("sec-websocket-key:") {
            return Some(line.split_once(':')?.1.trim().to_string());
        }
    }
    None
}

fn ws_accept(key: &str) -> String {
    let input = format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let hash = sha1(input.as_bytes());
    b64(&hash)
}

fn sha1(msg: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let bit_len = (msg.len() as u64) * 8;
    let mut padded = msg.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 { padded.push(0); }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3]]);
        }
        for i in 16..80 {
            w[i] = (w[i-3] ^ w[i-8] ^ w[i-14] ^ w[i-16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(w[i]);
            e = d; d = c; c = b.rotate_left(30); b = a; a = t;
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d); h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, &v) in h.iter().enumerate() { out[i*4..i*4+4].copy_from_slice(&v.to_be_bytes()); }
    out
}

fn b64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut r = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        r.push(T[((n >> 18) & 0x3F) as usize] as char);
        r.push(T[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 { r.push(T[((n >> 6) & 0x3F) as usize] as char); } else { r.push('='); }
        if chunk.len() > 2 { r.push(T[(n & 0x3F) as usize] as char); } else { r.push('='); }
    }
    r
}
