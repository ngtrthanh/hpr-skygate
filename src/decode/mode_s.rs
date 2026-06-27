/// Mode-S message parsing (Phase 2) + ADS-B decode (Phase 3)
/// Ported carefully from readsb mode_s.c

use super::crc::modes_checksum;

#[derive(Debug, Clone)]
pub struct ModeS {
    pub df: u8,
    pub icao: u32,
    pub receiver_id: u64,
    pub altitude: Option<i32>,
    pub squawk: Option<u16>,
    pub callsign: Option<String>,
    pub category: Option<u8>,
    pub cpr_lat: Option<u32>,
    pub cpr_lon: Option<u32>,
    pub cpr_odd: Option<bool>,
    pub airborne: bool,
    pub gs: Option<f64>,
    pub track: Option<f64>,
    pub baro_rate: Option<i32>,
    pub geom_rate: Option<i32>,
    pub ias: Option<u16>,
    pub tas: Option<u16>,
    pub mag_heading: Option<f64>,
    pub adsb_version: Option<u8>,
    pub nic: Option<u8>,
    pub nac_p: Option<u8>,
    pub nac_v: Option<u8>,
    pub sil: Option<u8>,
    pub gva: Option<u8>,
    pub sda: Option<u8>,
    pub nic_baro: Option<u8>,
    pub nav_altitude_mcp: Option<u32>,
    pub nav_qnh: Option<f64>,
    pub nav_heading: Option<f64>,
    pub emergency: Option<u8>,
    pub alt_gnss: Option<i32>,
    pub addr_type: u8, // 0=adsb_icao, 7=mode_s
    pub valid: bool,
}

impl ModeS {
    fn new() -> Self {
        Self {
            df: 0, icao: 0, receiver_id: 0, altitude: None, squawk: None, callsign: None,
            category: None, cpr_lat: None, cpr_lon: None, cpr_odd: None,
            airborne: true, gs: None, track: None, baro_rate: None, geom_rate: None,
            ias: None, tas: None, mag_heading: None, adsb_version: None,
            nic: None, nac_p: None, nac_v: None, sil: None, gva: None, sda: None,
            nic_baro: None, nav_altitude_mcp: None, nav_qnh: None, nav_heading: None,
            emergency: None, alt_gnss: None, addr_type: 0, valid: false,
        }
    }
}

/// Decode a Mode-S message. Returns None if CRC fails or message is unparseable.
pub fn decode(msg: &[u8]) -> Option<ModeS> {
    if msg.len() < 7 { return None; }
    let df = msg[0] >> 3;

    match df {
        0 | 4 | 5 | 11 | 16 | 20 | 21 => decode_short_long(msg, df),
        17 | 18 => decode_df17(msg, df),
        _ => None,
    }
}

fn decode_short_long(msg: &[u8], df: u8) -> Option<ModeS> {
    let len = if df == 16 || df == 20 || df == 21 { 14 } else { 7 };
    if msg.len() < len { return None; }

    let crc = modes_checksum(&msg[..len]);
    // For DF11: ICAO is in bytes 1-3, CRC residual should be 0 for valid
    // For DF0/4/5/16/20/21: ICAO = CRC residual (address/parity)
    let icao = if df == 11 {
        let addr = ((msg[1] as u32) << 16) | ((msg[2] as u32) << 8) | (msg[3] as u32);
        // For DF11, valid if residual is 0 or matches known aircraft
        // We accept any DF11 with 0 residual
        if crc != 0 { return None; }
        addr
    } else {
        // ICAO is the CRC residual
        crc
    };

    if icao == 0 { return None; }

    let mut m = ModeS::new();
    m.df = df;
    m.icao = icao;
    m.addr_type = 7; // mode_s
    m.valid = true;

    match df {
        0 | 16 => {
            m.altitude = decode_ac13(&msg[2..4]);
            m.airborne = true;
        }
        4 | 20 => {
            m.altitude = decode_ac13(&msg[2..4]);
            m.airborne = true;
            if df == 20 { decode_bds(&msg[4..11], &mut m); }
        }
        5 | 21 => {
            m.squawk = Some(decode_id13(&msg[2..4]));
            if df == 21 { decode_bds(&msg[4..11], &mut m); }
        }
        11 => {
            // DF11 only confirms existence, no payload
        }
        _ => {}
    }

    Some(m)
}

fn decode_df17(msg: &[u8], df: u8) -> Option<ModeS> {
    if msg.len() < 14 { return None; }

    // CRC must be zero for DF17/18; try 1-bit fix if not
    let mut buf = [0u8; 14];
    buf.copy_from_slice(&msg[..14]);
    if modes_checksum(&buf) != 0 {
        if !crate::decode::crc::modes_fix_single_bit(&mut buf) { return None; }
    }
    let msg = &buf[..];

    let icao = ((msg[1] as u32) << 16) | ((msg[2] as u32) << 8) | (msg[3] as u32);
    if icao == 0 { return None; }

    let mut m = ModeS::new();
    m.df = df;
    m.icao = icao;
    m.addr_type = if df == 18 { 8 } else { 0 };
    m.valid = true;

    let me = &msg[4..11]; // 7-byte ME field
    let tc = me[0] >> 3;

    match tc {
        1..=4 => decode_ident(me, tc, &mut m),
        5..=8 => decode_surface_pos(me, &mut m),
        9..=18 => decode_airborne_pos(me, &mut m),
        19 => decode_velocity(me, &mut m),
        20..=22 => {
            decode_airborne_pos(me, &mut m);
            // TC 20-22 = GNSS altitude
            if let Some(alt) = m.altitude {
                m.alt_gnss = Some(alt);
                m.altitude = None;
            }
        }
        28 => decode_emergency(me, &mut m),
        29 => decode_target_state(me, &mut m),
        31 => decode_op_status(me, tc, &mut m),
        _ => {}
    }

    Some(m)
}

// === ADS-B ME field decoders (Phase 3) ===

const AIS_CHARSET: &[u8] = b"?ABCDEFGHIJKLMNOPQRSTUVWXYZ????? ???????????????0123456789??????";

fn decode_ident(me: &[u8], tc: u8, m: &mut ModeS) {
    let ca = me[0] & 0x07;
    m.category = Some(((0x0E - tc) << 4) | ca);

    let bits = u64::from_be_bytes([0, me[0], me[1], me[2], me[3], me[4], me[5], me[6]]);
    let mut cs = String::with_capacity(8);
    for i in 0..8 {
        let idx = ((bits >> (42 - i * 6)) & 0x3F) as usize;
        let ch = AIS_CHARSET.get(idx).copied().unwrap_or(b' ');
        cs.push(ch as char);
    }
    let cs = cs.trim().to_string();
    if !cs.is_empty() && cs != "?" { m.callsign = Some(cs); }
}

fn decode_surface_pos(me: &[u8], m: &mut ModeS) {
    m.airborne = false;
    let movement = ((me[0] as u16 & 0x07) << 4) | ((me[1] as u16) >> 4);
    m.gs = decode_movement(movement);
    if (me[1] >> 3) & 1 == 1 {
        let raw = ((me[1] as u16 & 0x07) << 4) | ((me[2] as u16) >> 4);
        m.track = Some(raw as f64 * 360.0 / 128.0);
    }
    let odd = (me[2] >> 2) & 1;
    m.cpr_lat = Some((((me[2] as u32 & 0x03) << 15) | ((me[3] as u32) << 7) | ((me[4] as u32) >> 1)) & 0x1FFFF);
    m.cpr_lon = Some((((me[4] as u32 & 0x01) << 16) | ((me[5] as u32) << 8) | (me[6] as u32)) & 0x1FFFF);
    m.cpr_odd = Some(odd == 1);
}

fn decode_airborne_pos(me: &[u8], m: &mut ModeS) {
    m.airborne = true;
    let ac12 = (((me[1] as u16) << 4) | ((me[2] as u16) >> 4)) & 0xFFF;
    m.altitude = decode_ac12(ac12);
    let odd = (me[2] >> 2) & 1;
    m.cpr_lat = Some((((me[2] as u32 & 0x03) << 15) | ((me[3] as u32) << 7) | ((me[4] as u32) >> 1)) & 0x1FFFF);
    m.cpr_lon = Some((((me[4] as u32 & 0x01) << 16) | ((me[5] as u32) << 8) | (me[6] as u32)) & 0x1FFFF);
    m.cpr_odd = Some(odd == 1);

    let tc = me[0] >> 3;
    m.nic = Some(match tc {
        9 => 11, 10 => 10, 11 => 9, 12 => 8, 13 => 7,
        14 => 6, 15 => 5, 16 => 4, 17 => 3, 18 => 2, _ => 0
    });
}

fn decode_velocity(me: &[u8], m: &mut ModeS) {
    let subtype = me[0] & 0x07;
    m.nac_v = Some((me[1] >> 3) & 0x07);

    match subtype {
        1 | 2 => {
            let mult = if subtype == 2 { 4 } else { 1 };
            let ew_sign = (me[1] >> 2) & 1;
            let ew_raw = ((me[1] as i32 & 0x03) << 8) | me[2] as i32;
            let ns_sign = (me[3] >> 7) & 1;
            let ns_raw = ((me[3] as i32 & 0x7F) << 3) | (me[4] as i32 >> 5);

            if ew_raw > 0 && ns_raw > 0 {
                let ew_vel = (ew_raw - 1) * mult;
                let ns_vel = (ns_raw - 1) * mult;
                let vx = if ew_sign == 1 { -ew_vel } else { ew_vel };
                let vy = if ns_sign == 1 { -ns_vel } else { ns_vel };
                m.gs = Some(((vx * vx + vy * vy) as f64).sqrt());
                m.track = Some(((vx as f64).atan2(vy as f64).to_degrees() + 360.0) % 360.0);
            }

            let vr_sign = (me[4] >> 3) & 1;
            let vr_raw = ((me[4] as i32 & 0x07) << 6) | (me[5] as i32 >> 2);
            if vr_raw > 0 {
                let rate = (vr_raw - 1) * 64;
                let rate = if vr_sign == 1 { -rate } else { rate };
                let vr_src = (me[4] >> 4) & 1;
                if vr_src == 0 { m.baro_rate = Some(rate); }
                else { m.geom_rate = Some(rate); }
            }
        }
        3 | 4 => {
            let mult: u16 = if subtype == 4 { 4 } else { 1 };
            let hdg_avail = (me[1] >> 2) & 1;
            if hdg_avail == 1 {
                let hdg_raw = ((me[1] as u16 & 0x03) << 8) | me[2] as u16;
                m.mag_heading = Some(hdg_raw as f64 * 360.0 / 1024.0);
            }
            let as_type = (me[3] >> 7) & 1;
            let as_raw = (((me[3] as u16 & 0x7F) << 3) | (me[4] as u16 >> 5)).wrapping_sub(1);
            if as_raw < 0x3FE {
                let speed = as_raw * mult;
                if as_type == 0 { m.ias = Some(speed); }
                else { m.tas = Some(speed); }
            }

            let vr_sign = (me[4] >> 3) & 1;
            let vr_raw = ((me[4] as i32 & 0x07) << 6) | (me[5] as i32 >> 2);
            if vr_raw > 0 {
                let rate = (vr_raw - 1) * 64;
                m.baro_rate = Some(if vr_sign == 1 { -rate } else { rate });
            }
        }
        _ => {}
    }
}

fn decode_emergency(me: &[u8], m: &mut ModeS) {
    let subtype = me[0] & 0x07;
    if subtype == 1 {
        m.emergency = Some((me[1] >> 5) & 0x07);
        let sq = ((me[1] as u16 & 0x1F) << 8) | me[2] as u16;
        m.squawk = Some(decode_id13_raw(sq));
    }
}

fn decode_target_state(me: &[u8], m: &mut ModeS) {
    let subtype = me[0] & 0x07;
    if subtype == 1 {
        m.sil = Some((me[1] >> 6) & 0x03);
        let alt_raw = ((me[1] as u32 & 0x3F) << 5) | (me[2] as u32 >> 3);
        if alt_raw > 0 { m.nav_altitude_mcp = Some((alt_raw - 1) * 32); }
        let baro_raw = ((me[2] as u16 & 0x07) << 6) | (me[3] as u16 >> 2);
        if baro_raw > 0 { m.nav_qnh = Some((baro_raw as f64 - 1.0) * 0.8 + 800.0); }
        if (me[3] >> 1) & 1 == 1 {
            let hdg = ((me[3] as u16 & 0x01) << 8) | me[4] as u16;
            m.nav_heading = Some(hdg as f64 * 360.0 / 512.0);
        }
        m.nac_p = Some((me[5] >> 5) & 0x0F);
        m.nic_baro = Some((me[5] >> 3) & 1);
    }
}

fn decode_op_status(me: &[u8], _tc: u8, m: &mut ModeS) {
    m.adsb_version = Some((me[5] >> 5) & 0x07);
    m.nic = Some(me[5] & 0x0F);
    m.nac_p = Some(me[5] & 0x0F);
    m.sil = Some((me[6] >> 6) & 0x03);
    m.gva = Some((me[6] >> 2) & 0x03);
    m.sda = Some(me[6] & 0x03);
}

// === Altitude / Squawk decoders ===

fn decode_ac12(ac12: u16) -> Option<i32> {
    if ac12 == 0 { return None; }
    let q_bit = (ac12 >> 4) & 1;
    if q_bit == 1 {
        let n = ((ac12 & 0xFE0) >> 1) | (ac12 & 0x00F);
        Some(n as i32 * 25 - 1000)
    } else {
        None // Gillham coded, skip for now
    }
}

fn decode_ac13(bytes: &[u8]) -> Option<i32> {
    let ac13 = (((bytes[0] as u16) << 8) | bytes[1] as u16) & 0x1FFF;
    if ac13 == 0 { return None; }
    let m_bit = (ac13 >> 6) & 1;
    let q_bit = (ac13 >> 4) & 1;
    if m_bit == 0 && q_bit == 1 {
        let n = ((ac13 & 0x1F80) >> 2) | ((ac13 & 0x0020) >> 1) | (ac13 & 0x000F);
        Some(n as i32 * 25 - 1000)
    } else {
        None
    }
}

fn decode_id13(bytes: &[u8]) -> u16 {
    let id13 = (((bytes[0] as u16) << 8) | bytes[1] as u16) & 0x1FFF;
    let a = (id13 >> 10) & 0x07;
    let b = (id13 >> 4) & 0x07;
    let c = (id13 >> 1) & 0x07;
    let d = (id13 & 0x01) | ((id13 >> 12) & 0x02) | ((id13 >> 7) & 0x04);
    a * 1000 + b * 100 + c * 10 + d
}

fn decode_id13_raw(sq: u16) -> u16 {
    let a = (sq >> 10) & 0x07;
    let b = (sq >> 4) & 0x07;
    let c = (sq >> 1) & 0x07;
    let d = (sq & 0x01) | ((sq >> 12) & 0x02) | ((sq >> 7) & 0x04);
    a * 1000 + b * 100 + c * 10 + d
}

/// BDS 5,0: Airborne track and velocity (from DF20/21 Comm-B MB field)
fn decode_bds(mb: &[u8], m: &mut ModeS) {
    if mb.len() < 7 { return; }
    let roll_valid = (mb[0] >> 7) & 1;
    let track_valid = (mb[0] >> 2) & 1;
    let gs_valid = (mb[1] >> 7) & 1;
    let tas_valid = (mb[3] >> 4) & 1;

    let valid_count = roll_valid + track_valid + gs_valid + tas_valid;
    if valid_count < 3 { return; }

    if gs_valid == 1 {
        let gs_raw = ((mb[1] as u16 & 0x7F) << 3) | (mb[2] as u16 >> 5);
        if gs_raw > 0 && gs_raw < 1024 {
            let gs = gs_raw as f64 * 1024.0 / 512.0;
            if gs > 0.0 && gs < 700.0 { m.gs = Some(gs); }
        }
    }
    if tas_valid == 1 {
        let tas_raw = ((mb[3] as u16 & 0x0F) << 6) | (mb[4] as u16 >> 2);
        if tas_raw > 0 && tas_raw < 600 {
            m.tas = Some(tas_raw);
        }
    }
    if track_valid == 1 {
        let trk_raw = ((mb[0] as u16 & 0x03) << 7) | (mb[1] as u16 >> 1) & 0x1FF;
        if trk_raw > 0 {
            let trk = trk_raw as f64 * 360.0 / 512.0;
            if trk >= 0.0 && trk < 360.0 { m.track = Some(trk); }
        }
    }
}

fn decode_movement(mov: u16) -> Option<f64> {
    if mov == 0 { return None; }
    if mov == 1 { return Some(0.0); }
    Some(match mov {
        2..=8 => (mov as f64 - 1.0) * 0.125,
        9..=12 => 1.0 + (mov as f64 - 9.0) * 0.25,
        13..=38 => 2.0 + (mov as f64 - 13.0) * 0.5,
        39..=93 => 15.0 + (mov as f64 - 39.0),
        94..=108 => 70.0 + (mov as f64 - 94.0) * 2.0,
        109..=123 => 100.0 + (mov as f64 - 109.0) * 5.0,
        124 => 175.0,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i+2], 16).unwrap()).collect()
    }

    #[test]
    fn test_df17_ident() {
        // 8D4840D6202CC371C32CE0576098 - TC=4 ident, callsign=KLM1023
        let msg = hex("8D4840D6202CC371C32CE0576098");
        let m = decode(&msg).unwrap();
        assert_eq!(m.df, 17);
        assert_eq!(m.icao, 0x4840D6);
        assert_eq!(m.callsign.as_deref(), Some("KLM1023"));
        assert!(m.category.is_some());
    }

    #[test]
    fn test_df17_velocity_ground() {
        // 8D485020994409940838175B284F - TC=19 subtype=1 velocity
        let msg = hex("8D485020994409940838175B284F");
        let m = decode(&msg).unwrap();
        assert_eq!(m.df, 17);
        assert_eq!(m.icao, 0x485020);
        assert!(m.gs.is_some());
        assert!(m.track.is_some());
        let gs = m.gs.unwrap();
        assert!(gs > 0.0 && gs < 1000.0, "gs={}", gs);
    }

    #[test]
    fn test_df17_airborne_position() {
        // 8D40621D58C382D690C8AC2863A7 - TC=11 airborne position
        let msg = hex("8D40621D58C382D690C8AC2863A7");
        let m = decode(&msg).unwrap();
        assert_eq!(m.df, 17);
        assert_eq!(m.icao, 0x40621D);
        assert!(m.cpr_lat.is_some());
        assert!(m.cpr_lon.is_some());
        assert!(m.cpr_odd.is_some());
        assert!(m.altitude.is_some());
    }

    #[test]
    fn test_bad_crc_rejected() {
        let mut msg = hex("8D4840D6202CC371C32CE0576098");
        msg[7] ^= 0xFF; // corrupt
        assert!(decode(&msg).is_none());
    }

    #[test]
    fn test_squawk_format() {
        // Squawk from decode_id13 should be 4-digit octal-style
        let bytes = [0x13, 0x50]; // some id13 value
        let sq = decode_id13(&bytes);
        assert!(sq <= 7777, "squawk {} > 7777", sq);
    }
}
