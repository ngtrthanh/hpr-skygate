use std::collections::HashMap;
use std::time::Instant;

pub const POSITION_SIZE: usize = 19;
pub const STATIC_SIZE: usize = 44;
pub const ATON_SIZE: usize = 34;

pub enum AisFrame {
    Position { data: [u8; POSITION_SIZE], mmsi: u32 },
    Static { data: [u8; STATIC_SIZE], mmsi: u32, name: String, callsign: String, ship_type: u8 },
    AtoN { data: [u8; ATON_SIZE], mmsi: u32 },
}

pub fn payload_to_bits(payload: &str) -> Vec<u8> {
    let mut bits = Vec::with_capacity(payload.len() * 6);
    for b in payload.bytes() {
        let mut v = b - 48;
        if v > 40 { v -= 8; }
        for i in (0..6).rev() {
            bits.push((v >> i) & 1);
        }
    }
    bits
}

pub fn bits_uint(bits: &[u8], off: usize, len: usize) -> u32 {
    let mut val: u32 = 0;
    for i in 0..len {
        val = (val << 1) | (bits[off + i] as u32);
    }
    val
}

pub fn bits_signed(bits: &[u8], off: usize, len: usize) -> i32 {
    let raw = bits_uint(bits, off, len);
    if bits[off] == 1 {
        // sign extend
        let mask = 1u32 << len;
        (raw as i32) - (mask as i32)
    } else {
        raw as i32
    }
}

pub fn bits_text(bits: &[u8], off: usize, len: usize) -> String {
    let chars = len / 6;
    let mut s = String::with_capacity(chars);
    for i in 0..chars {
        let c = bits_uint(bits, off + i * 6, 6) as u8;
        let ch = if c < 32 { c + 64 } else { c };
        s.push(ch as char);
    }
    s.trim_end_matches('@').trim_end().to_string()
}

fn validate_position(lon_raw: i32, lat_raw: i32, mmsi: u32) -> bool {
    if mmsi == 0 { return false; }
    // AIS not-available: lon=0x6791AC0 (108600000), lat=0x3412140 (54600000)
    if lon_raw == 0x6791AC0i32 || lat_raw == 0x3412140i32 { return false; }
    if lon_raw == 0 && lat_raw == 0 { return false; }
    // 180° × 600000 = 108000000, 91° × 600000 = 54600000
    let lon_deg = (lon_raw as f64) / 600000.0;
    let lat_deg = (lat_raw as f64) / 600000.0;
    if lon_deg.abs() > 180.0 || lat_deg.abs() > 90.0 { return false; }
    true
}

fn encode_position(mmsi: u32, lon: i32, lat: i32, sog: u16, cog: u16, hdg: u16) -> [u8; POSITION_SIZE] {
    let mut d = [0u8; POSITION_SIZE];
    d[0] = 0x01;
    d[1..5].copy_from_slice(&mmsi.to_le_bytes());
    d[5..9].copy_from_slice(&lon.to_le_bytes());
    d[9..13].copy_from_slice(&lat.to_le_bytes());
    d[13..15].copy_from_slice(&sog.to_le_bytes());
    d[15..17].copy_from_slice(&cog.to_le_bytes());
    d[17..19].copy_from_slice(&hdg.to_le_bytes());
    d
}

fn encode_static(mmsi: u32, ship_type: u8, name: &str, callsign: &str, imo: u32, bow: u16, stern: u16, port: u8, starboard: u8) -> [u8; STATIC_SIZE] {
    let mut d = [0u8; STATIC_SIZE];
    d[0] = 0x05;
    d[1..5].copy_from_slice(&mmsi.to_le_bytes());
    d[5] = ship_type;
    // name: 20 bytes space-padded
    let name_bytes = name.as_bytes();
    let mut name_buf = [b' '; 20];
    let n = name_bytes.len().min(20);
    name_buf[..n].copy_from_slice(&name_bytes[..n]);
    d[6..26].copy_from_slice(&name_buf);
    // callsign: 7 bytes space-padded
    let cs_bytes = callsign.as_bytes();
    let mut cs_buf = [b' '; 7];
    let c = cs_bytes.len().min(7);
    cs_buf[..c].copy_from_slice(&cs_bytes[..c]);
    d[26..33].copy_from_slice(&cs_buf);
    d[33..37].copy_from_slice(&imo.to_le_bytes());
    d[37..39].copy_from_slice(&bow.to_le_bytes());
    d[39..41].copy_from_slice(&stern.to_le_bytes());
    d[41] = port;
    d[42] = starboard;
    // d[43] unused/padding
    d
}

fn encode_aton(mmsi: u32, aton_type: u8, lon: i32, lat: i32, name: &str) -> [u8; ATON_SIZE] {
    let mut d = [0u8; ATON_SIZE];
    d[0] = 0x15;
    d[1..5].copy_from_slice(&mmsi.to_le_bytes());
    d[5] = aton_type;
    d[6..10].copy_from_slice(&lon.to_le_bytes());
    d[10..14].copy_from_slice(&lat.to_le_bytes());
    let name_bytes = name.as_bytes();
    let mut name_buf = [b' '; 20];
    let n = name_bytes.len().min(20);
    name_buf[..n].copy_from_slice(&name_bytes[..n]);
    d[14..34].copy_from_slice(&name_buf);
    d
}

pub fn decode_ais(payload: &str) -> Option<AisFrame> {
    let bits = payload_to_bits(payload);
    if bits.len() < 38 { return None; }
    let msg_type = bits_uint(&bits, 0, 6);
    let mmsi = bits_uint(&bits, 8, 30);
    if mmsi == 0 { return None; }

    match msg_type {
        1 | 2 | 3 => decode_pos_123(&bits, mmsi),
        5 => decode_static_5(&bits, mmsi),
        18 => decode_pos_18(&bits, mmsi),
        19 => decode_pos_19(&bits, mmsi),
        21 => decode_aton_21(&bits, mmsi),
        24 => decode_static_24(&bits, mmsi),
        27 => decode_pos_27(&bits, mmsi),
        _ => None,
    }
}

fn decode_pos_123(bits: &[u8], mmsi: u32) -> Option<AisFrame> {
    if bits.len() < 168 { return None; }
    let sog_raw = bits_uint(bits, 50, 10) as u16; // 1/10 knot
    let lon_raw = bits_signed(bits, 61, 28);
    let lat_raw = bits_signed(bits, 89, 27);
    let cog_raw = bits_uint(bits, 116, 12) as u16; // 1/10 degree
    let hdg = bits_uint(bits, 128, 9) as u16;
    if !validate_position(lon_raw, lat_raw, mmsi) { return None; }
    let data = encode_position(mmsi, lon_raw, lat_raw, sog_raw, cog_raw, hdg);
    Some(AisFrame::Position { data, mmsi })
}

fn decode_static_5(bits: &[u8], mmsi: u32) -> Option<AisFrame> {
    if bits.len() < 424 { return None; }
    let imo = bits_uint(bits, 40, 30);
    let callsign = bits_text(bits, 70, 42);
    let name = bits_text(bits, 112, 120);
    let ship_type = bits_uint(bits, 232, 8) as u8;
    let bow = bits_uint(bits, 240, 9) as u16;
    let stern = bits_uint(bits, 249, 9) as u16;
    let port = bits_uint(bits, 258, 6) as u8;
    let starboard = bits_uint(bits, 264, 6) as u8;
    let data = encode_static(mmsi, ship_type, &name, &callsign, imo, bow, stern, port, starboard);
    Some(AisFrame::Static { data, mmsi, name, callsign, ship_type })
}

fn decode_pos_18(bits: &[u8], mmsi: u32) -> Option<AisFrame> {
    if bits.len() < 168 { return None; }
    let sog_raw = bits_uint(bits, 46, 10) as u16;
    let lon_raw = bits_signed(bits, 57, 28);
    let lat_raw = bits_signed(bits, 85, 27);
    let cog_raw = bits_uint(bits, 112, 12) as u16;
    let hdg = bits_uint(bits, 124, 9) as u16;
    if !validate_position(lon_raw, lat_raw, mmsi) { return None; }
    let data = encode_position(mmsi, lon_raw, lat_raw, sog_raw, cog_raw, hdg);
    Some(AisFrame::Position { data, mmsi })
}

fn decode_pos_19(bits: &[u8], mmsi: u32) -> Option<AisFrame> {
    if bits.len() < 312 { return None; }
    let sog_raw = bits_uint(bits, 46, 10) as u16;
    let lon_raw = bits_signed(bits, 57, 28);
    let lat_raw = bits_signed(bits, 85, 27);
    let cog_raw = bits_uint(bits, 112, 12) as u16;
    let hdg = bits_uint(bits, 124, 9) as u16;
    if !validate_position(lon_raw, lat_raw, mmsi) { return None; }
    let data = encode_position(mmsi, lon_raw, lat_raw, sog_raw, cog_raw, hdg);
    Some(AisFrame::Position { data, mmsi })
}

fn decode_aton_21(bits: &[u8], mmsi: u32) -> Option<AisFrame> {
    if bits.len() < 272 { return None; }
    let aton_type = bits_uint(bits, 38, 5) as u8;
    let name = bits_text(bits, 43, 120);
    let lon_raw = bits_signed(bits, 164, 28);
    let lat_raw = bits_signed(bits, 192, 27);
    if !validate_position(lon_raw, lat_raw, mmsi) { return None; }
    let data = encode_aton(mmsi, aton_type, lon_raw, lat_raw, &name);
    Some(AisFrame::AtoN { data, mmsi })
}

fn decode_static_24(bits: &[u8], mmsi: u32) -> Option<AisFrame> {
    if bits.len() < 160 { return None; }
    let part = bits_uint(bits, 38, 2);
    match part {
        0 => {
            // Part A: name only
            let name = bits_text(bits, 40, 120);
            let data = encode_static(mmsi, 0, &name, "", 0, 0, 0, 0, 0);
            Some(AisFrame::Static { data, mmsi, name, callsign: String::new(), ship_type: 0 })
        }
        1 => {
            if bits.len() < 168 { return None; }
            let ship_type = bits_uint(bits, 40, 8) as u8;
            let callsign = bits_text(bits, 90, 42);
            let bow = bits_uint(bits, 132, 9) as u16;
            let stern = bits_uint(bits, 141, 9) as u16;
            let port = bits_uint(bits, 150, 6) as u8;
            let starboard = bits_uint(bits, 156, 6) as u8;
            let data = encode_static(mmsi, ship_type, "", &callsign, 0, bow, stern, port, starboard);
            Some(AisFrame::Static { data, mmsi, name: String::new(), callsign, ship_type })
        }
        _ => None,
    }
}

fn decode_pos_27(bits: &[u8], mmsi: u32) -> Option<AisFrame> {
    if bits.len() < 96 { return None; }
    let sog_raw = (bits_uint(bits, 79, 6) as u16) * 10; // type27 sog is in knots, scale to 1/10
    let lon_raw = bits_signed(bits, 44, 18) * 600; // type27 lon in 1/10 min, convert to 1/10000 min
    let lat_raw = bits_signed(bits, 62, 17) * 600;
    let cog_raw = (bits_uint(bits, 85, 9) as u16) * 10; // degrees -> 1/10 degree
    let hdg = 511u16; // not available in type 27
    if !validate_position(lon_raw, lat_raw, mmsi) { return None; }
    let data = encode_position(mmsi, lon_raw, lat_raw, sog_raw, cog_raw, hdg);
    Some(AisFrame::Position { data, mmsi })
}

// --- Fragment Assembler ---

pub struct FragmentAssembler {
    fragments: HashMap<String, (Vec<Option<String>>, Instant)>,
}

impl FragmentAssembler {
    pub fn new() -> Self {
        Self { fragments: HashMap::new() }
    }

    /// Returns assembled payload if all parts received, else None
    pub fn process(&mut self, sentence: &str) -> Option<String> {
        // Clean up old fragments
        self.fragments.retain(|_, (_, ts)| ts.elapsed().as_secs() < 30);

        // Parse: !AIVDM,total,part,seq_id,channel,payload,checksum
        let s = sentence.trim();
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() < 7 { return None; }
        // Strip the tag prefix (e.g. !AIVDM or !AIVDO)
        let total: usize = parts[1].parse().ok()?;
        let part_num: usize = parts[2].parse().ok()?;
        if part_num == 0 || total == 0 { return None; }

        let payload = parts[5];

        if total == 1 {
            return Some(payload.to_string());
        }

        // Multi-part: use seq_id + channel as key
        let seq_id = parts[3];
        let channel = parts[4];
        let key = format!("{}-{}", seq_id, channel);

        let entry = self.fragments.entry(key.clone()).or_insert_with(|| {
            (vec![None; total], Instant::now())
        });

        if entry.0.len() != total {
            // Mismatch, reset
            *entry = (vec![None; total], Instant::now());
        }

        entry.0[part_num - 1] = Some(payload.to_string());

        if entry.0.iter().all(|p| p.is_some()) {
            let assembled: String = entry.0.iter()
                .map(|p| p.as_ref().unwrap().as_str())
                .collect();
            self.fragments.remove(&key);
            Some(assembled)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_to_bits() {
        let bits = payload_to_bits("1");
        assert_eq!(bits.len(), 6);
        // '1' = 49 - 48 = 1 = 000001
        assert_eq!(&bits[..], &[0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn test_bits_uint() {
        let bits = vec![1, 0, 1, 0]; // = 10
        assert_eq!(bits_uint(&bits, 0, 4), 10);
    }

    #[test]
    fn test_bits_signed_negative() {
        // 11111111 = -1 in 8-bit signed
        let bits = vec![1, 1, 1, 1, 1, 1, 1, 1];
        assert_eq!(bits_signed(&bits, 0, 8), -1);
    }

    #[test]
    fn test_bits_text() {
        // 'A' in 6-bit AIS = 1 (0b000001), maps to 'A' (1 + 64 = 65)
        let bits = vec![0, 0, 0, 0, 0, 1];
        assert_eq!(bits_text(&bits, 0, 6), "A");
    }

    #[test]
    fn test_type1_position() {
        // !AIVDM,1,1,,B,177KQJ5000G?tO`K>RA1wUbN0TKH,0*5C
        let payload = "177KQJ5000G?tO`K>RA1wUbN0TKH";
        let frame = decode_ais(payload);
        assert!(frame.is_some());
        if let Some(AisFrame::Position { mmsi, data, .. }) = frame {
            assert_eq!(mmsi, 477553000);
            assert_eq!(data[0], 0x01);
        }
    }

    #[test]
    fn test_type5_static() {
        // Full type 5: 71 chars = 426 bits (424 needed)
        let payload = "55?MbV02>H97ac<H4eEK6WT4r0Th4000000000000000000000000000000000000000000";
        let bits = payload_to_bits(payload);
        assert!(bits.len() >= 424);
        let frame = decode_ais(payload);
        assert!(frame.is_some());
        if let Some(AisFrame::Static { mmsi, .. }) = frame {
            assert_eq!(mmsi, 351759000);
        }
    }

    #[test]
    fn test_fragment_assembler_single() {
        let mut asm = FragmentAssembler::new();
        let result = asm.process("!AIVDM,1,1,,A,15N4cJ`005Ip000,0*72");
        assert_eq!(result, Some("15N4cJ`005Ip000".to_string()));
    }

    #[test]
    fn test_fragment_assembler_multi() {
        let mut asm = FragmentAssembler::new();
        let r1 = asm.process("!AIVDM,2,1,3,B,55?MbV02>H97ac<H4eEK6WT4r0Th40,0*2A");
        assert!(r1.is_none());
        let r2 = asm.process("!AIVDM,2,2,3,B,00000000000P0`D247i0Ep4l`888888888880,2*2C");
        assert!(r2.is_some());
    }

    #[test]
    fn test_reject_zero_position() {
        // Craft a type 1 with lon=0 lat=0 — should reject
        // mmsi=123456789, lon=0, lat=0
        // We'll just verify the validation logic directly
        assert!(!validate_position(0, 0, 123456789));
    }

    #[test]
    fn test_reject_not_available() {
        assert!(!validate_position(0x6791AC0, 0, 123456789));
        assert!(!validate_position(0, 0x3412140, 123456789));
    }

    #[test]
    fn test_reject_zero_mmsi() {
        assert!(!validate_position(1000, 1000, 0));
    }
}
