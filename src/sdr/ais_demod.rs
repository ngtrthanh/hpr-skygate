/// AIS 162 MHz IQ demodulator
/// FM discriminator → clock recovery → NRZI → HDLC deframe → NMEA sentence

const BAUD_RATE: f64 = 9600.0;
const HDLC_FLAG: u8 = 0x7E;

pub struct AisDemod {
    sample_rate: f64,
    samples_per_bit: f64,
    // FM demod state
    prev_i: f64,
    prev_q: f64,
    // Clock recovery (early-late gate)
    clock_phase: f64,
    clock_freq: f64,
    // Bit buffer
    shift_reg: u8,
    bit_count: u8,
    // HDLC deframe
    frame_buf: Vec<u8>,
    ones_count: u8,
    in_frame: bool,
    // Stats
    pub frames_ok: u64,
    pub frames_crc_fail: u64,
}

impl AisDemod {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f64;
        Self {
            sample_rate: sr,
            samples_per_bit: sr / BAUD_RATE,
            prev_i: 0.0,
            prev_q: 0.0,
            clock_phase: 0.0,
            clock_freq: sr / BAUD_RATE,
            shift_reg: 0,
            bit_count: 0,
            frame_buf: Vec::with_capacity(256),
            ones_count: 0,
            in_frame: false,
            frames_ok: 0,
            frames_crc_fail: 0,
        }
    }

    /// Process IQ chunk. Calls `emit` with each decoded NMEA sentence.
    pub fn process_chunk(&mut self, iq: &[u8], mut emit: impl FnMut(&str)) {
        let samples = iq.len() / 2;
        for s in 0..samples {
            let i = (iq[s * 2] as f64 - 127.5) / 128.0;
            let q = (iq[s * 2 + 1] as f64 - 127.5) / 128.0;

            // FM discriminator: phase difference between consecutive samples
            let demod = (q * self.prev_i - i * self.prev_q).atan2(i * self.prev_i + q * self.prev_q);
            self.prev_i = i;
            self.prev_q = q;

            // Clock recovery: sample at optimal point
            self.clock_phase += 1.0;
            if self.clock_phase >= self.clock_freq {
                self.clock_phase -= self.clock_freq;

                // Slice: positive = 1, negative = 0
                let bit = if demod > 0.0 { 1u8 } else { 0u8 };

                // NRZI decode: transition = 0, no transition = 1 (AIS standard)
                let nrzi_bit = if bit == ((self.shift_reg >> 7) & 1) { 1u8 } else { 0u8 };

                self.process_bit(nrzi_bit, &mut emit);
            }
        }
    }

    fn process_bit(&mut self, bit: u8, emit: &mut impl FnMut(&str)) {
        self.shift_reg = (self.shift_reg << 1) | bit;

        // Check for HDLC flag (0x7E = 01111110)
        if self.shift_reg == HDLC_FLAG {
            if self.in_frame && self.frame_buf.len() >= 5 {
                self.finish_frame(emit);
            }
            self.in_frame = true;
            self.frame_buf.clear();
            self.ones_count = 0;
            self.bit_count = 0;
            return;
        }

        if !self.in_frame { return; }

        // Bit-unstuffing: after 5 consecutive 1s, a 0 is stuffed (discard it)
        if bit == 1 {
            self.ones_count += 1;
            if self.ones_count > 6 {
                self.in_frame = false;
                return;
            }
        } else {
            if self.ones_count == 5 {
                self.ones_count = 0;
                return; // stuffed bit, discard
            }
            self.ones_count = 0;
        }

        // Accumulate bits into frame bytes
        self.bit_count += 1;
        if self.bit_count >= 8 {
            self.frame_buf.push(self.shift_reg);
            self.bit_count = 0;
            if self.frame_buf.len() > 512 {
                self.in_frame = false;
            }
        }
    }

    fn finish_frame(&mut self, emit: &mut impl FnMut(&str)) {
        if self.frame_buf.len() < 5 { return; }

        // Last 2 bytes are CRC-16 (CCITT)
        let payload_len = self.frame_buf.len() - 2;
        let payload = &self.frame_buf[..payload_len];
        let recv_crc = ((self.frame_buf[payload_len] as u16) << 8) | self.frame_buf[payload_len + 1] as u16;
        let calc_crc = crc16_ccitt(payload);

        if calc_crc != recv_crc {
            self.frames_crc_fail += 1;
            return;
        }

        self.frames_ok += 1;

        // Convert binary payload to 6-bit ASCII NMEA payload
        let nmea_payload = bits_to_nmea_payload(payload);
        if nmea_payload.is_empty() { return; }

        // Format as NMEA sentence
        let sentence = format!("!AIVDM,1,1,,A,{},0", nmea_payload);
        let checksum = nmea_checksum(&sentence[1..sentence.rfind('*').unwrap_or(sentence.len())]);
        let full = format!("{}*{:02X}", sentence.rsplit_once(',').unwrap().0, checksum);
        // Actually simpler: just emit with computed checksum
        let body = &format!("AIVDM,1,1,,A,{},0", nmea_payload);
        let cs = nmea_checksum(body);
        emit(&format!("!{}*{:02X}", body, cs));
    }
}

fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc ^ 0xFFFF
}

fn bits_to_nmea_payload(data: &[u8]) -> String {
    // AIS payload is 6-bit encoded: each byte of payload → one ASCII char
    // The binary frame IS the payload bits directly
    let total_bits = data.len() * 8;
    let chars = total_bits / 6;
    let mut out = String::with_capacity(chars);
    for i in 0..chars {
        let bit_offset = i * 6;
        let byte_idx = bit_offset / 8;
        let bit_in_byte = bit_offset % 8;
        let mut val: u8;
        if bit_in_byte <= 2 {
            val = (data[byte_idx] >> (2 - bit_in_byte)) & 0x3F;
        } else {
            val = (data[byte_idx] << (bit_in_byte - 2)) & 0x3F;
            if byte_idx + 1 < data.len() {
                val |= data[byte_idx + 1] >> (10 - bit_in_byte);
            }
        }
        // 6-bit to ASCII armor
        let c = if val < 40 { val + 48 } else { val + 56 };
        out.push(c as char);
    }
    out
}

fn nmea_checksum(s: &str) -> u8 {
    s.bytes().fold(0u8, |acc, b| acc ^ b)
}
