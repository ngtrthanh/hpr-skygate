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
    loop {
        let addr = match format!("{}:{}", host, port).to_socket_addrs() {
            Ok(mut a) => match a.next() {
                Some(a) => a,
                None => { std::thread::sleep(Duration::from_secs(5)); continue; }
            },
            Err(_) => { std::thread::sleep(Duration::from_secs(5)); continue; }
        };
        match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
            Ok(stream) => {
                tracing::info!(source = %name, "AIS connected to {}:{}", host, port);
                let reader = BufReader::new(stream);
                for line in reader.lines() {
                    let line = match line { Ok(l) => l, Err(_) => break };
                    let sentence = line.trim();
                    if !sentence.starts_with('!') { continue; }
                    if dedup.is_duplicate(sentence) { continue; }
                    if let Some(payload) = assembler.process(sentence) {
                        if let Some(frame) = decode::decode_ais(&payload) {
                            if tx.send(frame).is_err() { return; }
                        }
                    }
                }
                tracing::warn!(source = %name, "AIS disconnected, reconnecting...");
            }
            Err(e) => {
                tracing::warn!(source = %name, "AIS connect failed: {}", e);
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}
