/// Compact frame ingest from hpr-demod
/// Frame format: [len:u8][signal:u8][payload:7|14] or [len:u8][signal:u8][payload:7|14][confidence:u8]
/// len = 8|15 (no confidence) or 9|16 (with confidence). Receiver reads 1+len bytes total.

use std::io::Read;
use std::net::TcpListener;
use std::sync::mpsc::Sender;

use crate::decode::mode_s::{self, ModeS};

pub struct CompactFramer {
    buf: [u8; 256],
    pos: usize,
}

impl CompactFramer {
    pub fn new() -> Self {
        Self { buf: [0u8; 256], pos: 0 }
    }

    pub fn feed(&mut self, data: &[u8], mut emit: impl FnMut(u8, &[u8], Option<u8>)) {
        for &byte in data {
            self.buf[self.pos] = byte;
            self.pos += 1;
            if self.pos >= 1 {
                let frame_len = self.buf[0] as usize;
                let (payload_len, has_conf) = match frame_len {
                    8 => (7, false),
                    9 => (7, true),
                    15 => (14, false),
                    16 => (14, true),
                    _ => { self.pos = 0; continue; }
                };
                let wire_len = 1 + frame_len;
                if self.pos >= wire_len {
                    let signal = self.buf[1];
                    let payload = &self.buf[2..2 + payload_len];
                    let confidence = if has_conf { Some(self.buf[2 + payload_len]) } else { None };
                    emit(signal, payload, confidence);
                    self.pos = 0;
                }
            }
            if self.pos >= self.buf.len() {
                self.pos = 0;
            }
        }
    }
}

/// Spawn a blocking listener thread that accepts compact connections,
/// decodes frames, and sends ModeS messages over the channel.
pub fn spawn_compact_listener(port: u16, tx: Sender<ModeS>) {
    std::thread::spawn(move || {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).expect("bind demod port");
        tracing::info!(port, "compact ingest listening");
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut framer = CompactFramer::new();
                let mut buf = [0u8; 4096];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            framer.feed(&buf[..n], |_signal, payload, _confidence| {
                                if let Some(msg) = mode_s::decode(payload) {
                                    let _ = tx.send(msg);
                                }
                            });
                        }
                    }
                }
            });
        }
    });
}
