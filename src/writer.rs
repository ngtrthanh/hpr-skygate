use std::collections::VecDeque;
use std::io::{self, Write};
use std::net::{TcpStream, SocketAddr};
use std::time::{Duration, Instant};

const FLUSH_SIZE: usize = 1280;
const SENDQ_MAX: usize = 256 * 1024;
const HEARTBEAT: &[u8] = b"\x1a\x31\x00\x00\x00\x00\x00\x00\x00\x00\x00";

pub struct OutputWriter {
    buf: Vec<u8>,
    flush_size: usize,
    flush_interval: Duration,
    heartbeat_interval: Duration,
    last_flush: Instant,
    last_write: Instant,
    clients: Vec<SubClient>,
}

struct SubClient {
    stream: TcpStream,
    addr: SocketAddr,
    sendq: VecDeque<u8>,
    sendq_max: usize,
    drop_half: bool,
    drop_half_toggle: bool,
    drop_until: Instant,
    last_send: Instant,
    bytes_sent: u64,
    bytes_from_writer: u64,
    dead: bool,
}

impl OutputWriter {
    pub fn new(flush_interval_ms: u64, heartbeat_secs: u64) -> Self {
        Self {
            buf: Vec::with_capacity(FLUSH_SIZE * 2),
            flush_size: FLUSH_SIZE,
            flush_interval: Duration::from_millis(flush_interval_ms),
            heartbeat_interval: Duration::from_secs(heartbeat_secs),
            last_flush: Instant::now(),
            last_write: Instant::now(),
            clients: Vec::new(),
        }
    }

    /// Append frame data to shared buffer. Flushes if size threshold hit.
    pub fn append(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= self.flush_size {
            self.flush();
        }
    }

    /// Check timer-based flush + heartbeat. Call from main loop.
    pub fn check_flush(&mut self) {
        let now = Instant::now();
        if !self.buf.is_empty() && now.duration_since(self.last_flush) >= self.flush_interval {
            self.flush();
        }
        if now.duration_since(self.last_write) >= self.heartbeat_interval {
            self.buf.extend_from_slice(HEARTBEAT);
            self.flush();
        }
    }

    /// Flush shared buffer to all client sendqs, then try write.
    fn flush(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let now = Instant::now();
        let data_len = self.buf.len();

        for client in &mut self.clients {
            if client.dead {
                continue;
            }
            client.bytes_from_writer += data_len as u64;

            // Check if sendq can fit
            let insufficient = client.sendq.len() + data_len > client.sendq_max;
            if insufficient {
                client.start_drop_half(now);
            }

            // dropHalf logic: toggle drop each flush
            if client.drop_until > now {
                client.drop_half_toggle = !client.drop_half_toggle;
                if client.drop_half_toggle || insufficient {
                    continue; // skip this flush for this client
                }
            }

            // Copy data into client sendq
            client.sendq.extend(self.buf.iter());
            client.bytes_sent += data_len as u64;
        }

        self.buf.clear();
        self.last_flush = now;
        self.last_write = now;

        // Try to drain each client's sendq
        for client in &mut self.clients {
            if client.dead || client.sendq.is_empty() {
                continue;
            }
            client.try_flush(now);
        }

        // Remove dead clients
        self.clients.retain(|c| !c.dead);
    }

    pub fn add_client(&mut self, stream: TcpStream, addr: SocketAddr) {
        stream.set_nonblocking(true).ok();
        stream.set_nodelay(true).ok();
        self.clients.push(SubClient {
            stream,
            addr,
            sendq: VecDeque::with_capacity(32 * 1024),
            sendq_max: SENDQ_MAX,
            drop_half: false,
            drop_half_toggle: false,
            drop_until: Instant::now(),
            last_send: Instant::now(),
            bytes_sent: 0,
            bytes_from_writer: 0,
            dead: false,
        });
    }

    pub fn client_count(&self) -> usize {
        self.clients.len()
    }
}

impl SubClient {
    fn start_drop_half(&mut self, now: Instant) {
        self.drop_half = true;
        self.drop_half_toggle = true;
        self.drop_until = now + Duration::from_secs(2);
    }

    fn try_flush(&mut self, now: Instant) {
        let (slice1, slice2) = self.sendq.as_slices();
        let to_write = if !slice1.is_empty() { slice1 } else { slice2 };
        if to_write.is_empty() {
            return;
        }

        match (&self.stream).write(to_write) {
            Ok(0) => {
                self.dead = true;
            }
            Ok(n) => {
                self.sendq.drain(..n);
                self.last_send = now;
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Can't write now, leave in sendq
            }
            Err(_) => {
                self.dead = true;
            }
        }

        // Kill clients that haven't been able to send for 5s
        if now.duration_since(self.last_send) > Duration::from_secs(5) && !self.sendq.is_empty() {
            self.dead = true;
        }
    }
}
