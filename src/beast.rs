use bytes::{Bytes, BytesMut, BufMut};

pub const BEAST_ESCAPE: u8 = 0x1a;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    ModeAC,    // 0x31 — 9 bytes (6ts + 1sig + 2payload)
    ModeSShort,// 0x32 — 14 bytes
    ModeSLong, // 0x33 — 21 bytes
    Config,    // 0x34 — 21 bytes
    Extended,  // 0x35 — 22 bytes (14+8)
    ReceiverID,// 0xe3 — 8 bytes, no ts/sig
    UUID,      // 0xe4 — 36 ASCII bytes, no ts/sig
}

impl FrameType {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x31 => Some(Self::ModeAC),
            0x32 => Some(Self::ModeSShort),
            0x33 => Some(Self::ModeSLong),
            0x34 => Some(Self::Config),
            0x35 => Some(Self::Extended),
            0xe3 => Some(Self::ReceiverID),
            0xe4 => Some(Self::UUID),
            _ => None,
        }
    }

    /// Unescaped body length after the 0x1a + type byte.
    pub fn body_len(self) -> usize {
        match self {
            Self::ModeAC => 6 + 1 + 2,
            Self::ModeSShort => 6 + 1 + 7,
            Self::ModeSLong => 6 + 1 + 14,
            Self::Config => 6 + 1 + 14,
            Self::Extended => 14 + 8,
            Self::ReceiverID => 8,
            Self::UUID => 36,
        }
    }

    /// Whether body bytes use escape doubling.
    pub fn has_escaping(self) -> bool {
        !matches!(self, Self::UUID) // UUID is raw ASCII, no escaping
    }
}

/// Streaming Beast frame parser.
pub struct BeastFramer {
    buf: BytesMut,
}

impl BeastFramer {
    pub fn new() -> Self {
        Self { buf: BytesMut::with_capacity(8192) }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.put_slice(data);
    }

    /// Extract next complete frame as raw wire bytes (including 0x1a + type).
    pub fn next_frame(&mut self) -> Option<Bytes> {
        loop {
            let start = self.buf.iter().position(|&b| b == BEAST_ESCAPE)?;
            if start + 1 >= self.buf.len() {
                if start > 0 { let _ = self.buf.split_to(start); }
                return None;
            }

            let type_byte = self.buf[start + 1];
            let frame_type = match FrameType::from_byte(type_byte) {
                Some(ft) => ft,
                None => {
                    let _ = self.buf.split_to(start + 2);
                    continue;
                }
            };

            if start > 0 { let _ = self.buf.split_to(start); }

            let needed = frame_type.body_len();
            let mut wire_pos = 2;
            let mut count = 0;

            if frame_type.has_escaping() {
                while count < needed {
                    if wire_pos >= self.buf.len() { return None; }
                    if self.buf[wire_pos] == BEAST_ESCAPE {
                        if wire_pos + 1 >= self.buf.len() { return None; }
                        if self.buf[wire_pos + 1] == BEAST_ESCAPE {
                            wire_pos += 2;
                            count += 1;
                        } else {
                            // New frame start — current is truncated
                            let _ = self.buf.split_to(wire_pos);
                            break;
                        }
                    } else {
                        wire_pos += 1;
                        count += 1;
                    }
                }
            } else {
                // No escaping (UUID) — just need raw bytes
                if self.buf.len() < 2 + needed { return None; }
                wire_pos = 2 + needed;
                count = needed;
            }

            if count == needed {
                return Some(self.buf.split_to(wire_pos).freeze());
            }
        }
    }
}

/// Encode 0x1a 0xe3 [8-byte receiverId with escape doubling].
pub fn encode_e3_prefix(recv_id: u64) -> Bytes {
    let mut out = BytesMut::with_capacity(18);
    out.put_u8(BEAST_ESCAPE);
    out.put_u8(0xe3);
    for i in (0..8).rev() {
        let b = ((recv_id >> (i * 8)) & 0xff) as u8;
        out.put_u8(b);
        if b == BEAST_ESCAPE {
            out.put_u8(BEAST_ESCAPE);
        }
    }
    out.freeze()
}

/// Parse receiver ID from a 0xe3 frame's body (unescaped 8 bytes after 0x1a 0xe3).
pub fn parse_receiver_id(wire: &[u8]) -> Option<u64> {
    if wire.len() < 2 || wire[0] != BEAST_ESCAPE || wire[1] != 0xe3 {
        return None;
    }
    let mut id: u64 = 0;
    let mut pos = 2;
    for _ in 0..8 {
        if pos >= wire.len() { return None; }
        let b = wire[pos];
        pos += 1;
        if b == BEAST_ESCAPE {
            if pos >= wire.len() { return None; }
            pos += 1; // skip doubled escape
        }
        id = id << 8 | b as u64;
    }
    Some(id)
}

/// Extract UUID string from a 0xe4 frame.
pub fn parse_uuid_frame(wire: &[u8]) -> Option<&str> {
    if wire.len() < 38 || wire[0] != BEAST_ESCAPE || wire[1] != 0xe4 {
        return None;
    }
    std::str::from_utf8(&wire[2..38]).ok()
}
