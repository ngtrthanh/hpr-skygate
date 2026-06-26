use std::time::Instant;

use crate::beast::{BeastFramer, FrameType, encode_e3_prefix, parse_uuid_frame, parse_receiver_id};
use crate::receiver::{uuid_to_receiver_id, is_valid_uuid};

const RATE_LIMIT: u64 = 5420;
const RATE_WINDOW_SECS: u64 = 2;

/// Per-feeder connection state. Processes raw bytes into framed output.
pub struct ClientState {
    pub addr: String,
    pub uuid: Option<String>,
    pub receiver_id: u64,
    pub id_locked: bool,
    pub garbage: bool,
    pub msg_count: u64,
    pub bytes_recv: u64,

    framer: BeastFramer,
    rate_count: u64,
    rate_reset: Instant,
    e3_prefix: Option<Vec<u8>>,
}

impl ClientState {
    pub fn new(addr: String) -> Self {
        let now = Instant::now();
        Self {
            addr,
            uuid: None,
            receiver_id: 0,
            id_locked: false,
            garbage: false,
            msg_count: 0,
            bytes_recv: 0,
            framer: BeastFramer::new(),
            rate_count: 0,
            rate_reset: now + std::time::Duration::from_secs(RATE_WINDOW_SECS),
            e3_prefix: None,
        }
    }

    /// Feed raw TCP data. Calls `emit` for each routed frame.
    /// emit(frame_bytes, is_garbage)
    pub fn feed<F>(&mut self, data: &[u8], prepend_recv_id: bool, mut emit: F)
    where
        F: FnMut(&[u8], bool),
    {
        self.bytes_recv += data.len() as u64;
        self.framer.push(data);

        while let Some(frame) = self.framer.next_frame() {
            if frame.len() < 2 { continue; }
            let typ = FrameType::from_byte(frame[1]);

            match typ {
                Some(FrameType::UUID) => {
                    if !self.id_locked {
                        if let Some(uuid) = parse_uuid_frame(&frame) {
                            if is_valid_uuid(uuid) {
                                self.lock_uuid(uuid.to_string(), prepend_recv_id);
                            }
                        }
                    }
                    continue;
                }
                Some(FrameType::ReceiverID) => {
                    if let Some(id) = parse_receiver_id(&frame) {
                        if !self.id_locked {
                            self.receiver_id = id;
                            self.id_locked = true;
                            self.e3_prefix = Some(encode_e3_prefix(id).to_vec());
                        }
                    }
                    // Forward with existing E3 prefix
                    self.rate_check();
                    emit(&frame, self.garbage);
                    continue;
                }
                _ => {}
            }

            self.rate_check();

            // Emit with optional E3 prefix
            if prepend_recv_id {
                if let Some(ref prefix) = self.e3_prefix {
                    emit(prefix, self.garbage);
                }
            }
            emit(&frame, self.garbage);
        }
    }

    fn rate_check(&mut self) {
        self.msg_count += 1;
        self.rate_count += 1;
        let now = Instant::now();
        if now >= self.rate_reset {
            let rate = self.rate_count / RATE_WINDOW_SECS;
            self.garbage = rate > RATE_LIMIT;
            self.rate_count = 0;
            self.rate_reset = now + std::time::Duration::from_secs(RATE_WINDOW_SECS);
        }
    }

    fn lock_uuid(&mut self, uuid: String, prepend: bool) {
        self.receiver_id = uuid_to_receiver_id(&uuid);
        self.uuid = Some(uuid);
        self.id_locked = true;
        if prepend {
            self.e3_prefix = Some(encode_e3_prefix(self.receiver_id).to_vec());
        }
    }
}
