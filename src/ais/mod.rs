pub mod decode;
pub mod vessel;
pub mod dedup;

use std::io::{BufRead, BufReader};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use decode::{AisFrame, FragmentAssembler};
use dedup::DedupCache;

pub struct AisSource {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub connected: bool,
    pub received: u64,
    pub last_msg: Option<Instant>,
}

pub struct AisIngest {
    pub sources: Vec<AisSource>,
    dedup: DedupCache,
    assembler: FragmentAssembler,
}

impl AisIngest {
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            dedup: DedupCache::new(Duration::from_secs(30)),
            assembler: FragmentAssembler::new(),
        }
    }

    /// Parse "name1=host1:port1,name2=host2:port2" format
    pub fn parse_sources(config: &str) -> Vec<(String, String, u16)> {
        config.split(',')
            .filter_map(|entry| {
                let (name, addr) = entry.split_once('=')?;
                let (host, port_str) = addr.rsplit_once(':')?;
                let port: u16 = port_str.parse().ok()?;
                Some((name.to_string(), host.to_string(), port))
            })
            .collect()
    }
}

/// Spawn reader threads for each AIS source.
pub fn spawn_ais_readers(
    config: &str,
    tx: mpsc::Sender<AisFrame>,
) -> Vec<std::thread::JoinHandle<()>> {
    AisIngest::parse_sources(config)
        .into_iter()
        .map(|(name, host, port)| {
            let tx = tx.clone();
            std::thread::Builder::new()
                .name(format!("ais-{}", name))
                .spawn(move || reader_loop(name, host, port, tx))
                .expect("spawn ais reader")
        })
        .collect()
}

fn reader_loop(name: String, host: String, port: u16, tx: mpsc::Sender<AisFrame>) {
    let mut assembler = FragmentAssembler::new();
    let mut dedup = DedupCache::new(Duration::from_secs(30));
    let mut accepted: u64 = 0;
    let mut duplicates: u64 = 0;
    let mut invalid: u64 = 0;

    loop {
        let addr = match format!("{}:{}", host, port).to_socket_addrs() {
            Ok(mut a) => match a.next() {
                Some(a) => a,
                None => { std::thread::sleep(Duration::from_secs(5)); continue; }
            },
            Err(_) => { std::thread::sleep(Duration::from_secs(5)); continue; }
        };
        match TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
            Ok(stream) => {
                stream.set_read_timeout(Some(Duration::from_secs(60))).ok();
                tracing::info!(source = %name, "AIS connected to {}:{}", host, port);
                let reader = BufReader::with_capacity(64 * 1024, stream);
                for line in reader.lines() {
                    let line = match line { Ok(l) => l, Err(_) => break };
                    let norm = match normalize_nmea(&line) {
                        Some(n) => n,
                        None => { invalid += 1; continue; }
                    };
                    if dedup.is_duplicate(&norm) {
                        duplicates += 1;
                        continue;
                    }
                    accepted += 1;
                    if let Some(payload) = assembler.process(&norm) {
                        if let Some(frame) = decode::decode_ais(&payload) {
                            if tx.send(frame).is_err() { return; }
                        }
                    }
                }
                tracing::warn!(source = %name, %accepted, %duplicates, %invalid, "AIS disconnected, reconnecting...");
            }
            Err(e) => {
                tracing::warn!(source = %name, "AIS connect failed: {}", e);
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// Normalize NMEA sentence (from hpr-atlas battle-tested logic):
/// - Find first '!' or '$' (skip any leading junk/timestamps)
/// - Trim to checksum (*XX)
/// - Validate prefix (!AI, !BS, $)
fn normalize_nmea(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() { return None; }
    // Find start of NMEA sentence
    let start = line.find(|c| c == '!' || c == '$')?;
    let mut s = &line[start..];
    // Trim to checksum
    if let Some(star) = s.find('*') {
        if s.len() >= star + 3 {
            s = &s[..star + 3];
        }
    }
    // Validate prefix
    if !(s.starts_with("!AI") || s.starts_with("!BS") || s.starts_with("$")) {
        return None;
    }
    Some(s.to_string())
}
