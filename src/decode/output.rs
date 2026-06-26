/// Output serialization: JSON + binCraft with bbox filter

use super::aircraft::{Aircraft, Store};

// === JSON output ===

pub fn build_json(store: &Store, bbox: Option<(f64, f64, f64, f64)>) -> Vec<u8> {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let mut buf = Vec::with_capacity(store.map.len() * 200 + 128);
    buf.extend_from_slice(b"{\"now\":");
    buf.extend_from_slice(format!("{:.1}", t).as_bytes());
    buf.extend_from_slice(b",\"messages\":");
    itoa(&mut buf, store.messages_total as i64);
    buf.extend_from_slice(b",\"aircraft\":[");
    let mut first = true;
    for ac in store.map.values() {
        if !include_aircraft(ac, bbox) { continue; }
        if !first { buf.push(b','); }
        first = false;
        write_aircraft_json(&mut buf, ac, t);
    }
    buf.extend_from_slice(b"]}");
    buf
}

fn include_aircraft(ac: &Aircraft, bbox: Option<(f64, f64, f64, f64)>) -> bool {
    // Must have at least one useful field
    if ac.alt_baro.is_none() && ac.lat.is_none() && ac.squawk.is_none() && ac.flight.is_none() {
        return false;
    }
    if let Some((south, north, west, east)) = bbox {
        match (ac.lat, ac.lon) {
            (Some(lat), Some(lon)) => {
                if lat < south || lat > north { return false; }
                if west <= east {
                    if lon < west || lon > east { return false; }
                } else {
                    // wraps antimeridian
                    if lon < west && lon > east { return false; }
                }
            }
            _ => return false, // no position = exclude from bbox query
        }
    }
    true
}

fn write_aircraft_json(buf: &mut Vec<u8>, ac: &Aircraft, t: f64) {
    buf.push(b'{');
    jstr(buf, "hex", &format!("{:06x}", ac.hex)); buf.push(b',');
    if let Some(ref r) = ac.reg { jstr(buf, "r", r); buf.push(b','); }
    if let Some(ref t) = ac.typecode { jstr(buf, "t", t); buf.push(b','); }
    if let Some(ref route) = ac.route { jstr(buf, "route", route); buf.push(b','); }
    if let Some(ref f) = ac.flight { jstr(buf, "flight", f); buf.push(b','); }
    if ac.on_ground {
        buf.extend_from_slice(b"\"alt_baro\":\"ground\",");
    } else if let Some(v) = ac.alt_baro { jint(buf, "alt_baro", v); buf.push(b','); }
    if let Some(v) = ac.alt_geom { jint(buf, "alt_geom", v); buf.push(b','); }
    if let Some(v) = ac.gs { jfloat(buf, "gs", v, 1); buf.push(b','); }
    if let Some(v) = ac.track { jfloat(buf, "track", v, 1); buf.push(b','); }
    if let Some(v) = ac.baro_rate { jint(buf, "baro_rate", v); buf.push(b','); }
    if let Some(v) = ac.geom_rate { jint(buf, "geom_rate", v); buf.push(b','); }
    if let Some(v) = ac.squawk { jstr(buf, "squawk", &format!("{:04}", v)); buf.push(b','); }
    if let Some(v) = ac.category { jstr(buf, "category", &format!("{:02X}", v)); buf.push(b','); }
    if let (Some(lat), Some(lon)) = (ac.lat, ac.lon) {
        jfloat(buf, "lat", lat, 6); buf.push(b',');
        jfloat(buf, "lon", lon, 6); buf.push(b',');
    }
    if let Some(v) = ac.ias { jint(buf, "ias", v as i32); buf.push(b','); }
    if let Some(v) = ac.tas { jint(buf, "tas", v as i32); buf.push(b','); }
    if let Some(v) = ac.mag_heading { jfloat(buf, "mag_heading", v, 1); buf.push(b','); }
    jint(buf, "messages", ac.messages as i32); buf.push(b',');
    jfloat(buf, "seen", t - ac.seen, 1);
    if ac.seen_pos > 0.0 { buf.push(b','); jfloat(buf, "seen_pos", t - ac.seen_pos, 1); }
    buf.push(b'}');
}

// === binCraft output (112 bytes/ac, readsb compatible) ===

const STRIDE: usize = 112;

pub fn build_bincraft(store: &Store, bbox: Option<(f64, f64, f64, f64)>) -> Vec<u8> {
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let now_s = now_ms as f64 / 1000.0;
    let mut buf = Vec::with_capacity(STRIDE + store.map.len() * STRIDE);
    // Header placeholder
    buf.extend_from_slice(&[0u8; STRIDE]);
    let mut count = 0u32;
    for ac in store.map.values() {
        if !include_aircraft(ac, bbox) { continue; }
        buf.extend_from_slice(&encode_bincraft(ac, now_s));
        count += 1;
    }
    // Fill header
    write_u32(&mut buf, 0, now_ms as u32);
    write_u32(&mut buf, 4, (now_ms >> 32) as u32);
    write_u32(&mut buf, 8, STRIDE as u32);
    write_u32(&mut buf, 12, count);
    write_u32(&mut buf, 16, 314159); // magic
    write_u32(&mut buf, 28, store.messages_total as u32);
    write_u32(&mut buf, 40, 20250625); // version
    buf
}

fn encode_bincraft(ac: &Aircraft, now_s: f64) -> [u8; 112] {
    let mut r = [0u8; 112];
    write_u32(&mut r, 0, ac.hex);
    write_i32(&mut r, 4, ((now_s - ac.seen) * 10.0) as i32);

    if let (Some(lat), Some(lon)) = (ac.lat, ac.lon) {
        if ac.seen_pos > 0.0 && (now_s - ac.seen_pos) < 60.0 {
            write_i32(&mut r, 8, (lon * 1e6) as i32);
            write_i32(&mut r, 12, (lat * 1e6) as i32);
        }
    }
    if let Some(v) = ac.baro_rate { write_i16(&mut r, 16, (v as f64 / 8.0) as i16); }
    if let Some(v) = ac.geom_rate { write_i16(&mut r, 18, (v as f64 / 8.0) as i16); }
    if let Some(v) = ac.alt_baro { write_i16(&mut r, 20, (v as f64 / 25.0) as i16); }
    if let Some(v) = ac.alt_geom { write_i16(&mut r, 22, (v as f64 / 25.0) as i16); }
    if let Some(v) = ac.squawk { write_u16(&mut r, 32, v); }
    if let Some(v) = ac.gs { write_i16(&mut r, 34, (v * 10.0) as i16); }
    if let Some(v) = ac.track { write_i16(&mut r, 40, (v * 90.0) as i16); }
    if let Some(v) = ac.mag_heading { write_i16(&mut r, 44, (v * 90.0) as i16); }
    if let Some(v) = ac.tas { write_u16(&mut r, 56, v); }
    if let Some(v) = ac.ias { write_u16(&mut r, 58, v); }
    write_u16(&mut r, 62, ac.messages.min(65535) as u16);
    if let Some(v) = ac.category { r[64] = v; }
    if let Some(v) = ac.nic { r[65] = v; }

    // Validity flags
    let pos_fresh = ac.lat.is_some() && ac.seen_pos > 0.0 && (now_s - ac.seen_pos) < 60.0;
    let mut v73: u8 = 0;
    if ac.flight.is_some() { v73 |= 8; }
    if ac.alt_baro.is_some() { v73 |= 16; }
    if ac.alt_geom.is_some() { v73 |= 32; }
    if pos_fresh { v73 |= 64; }
    if ac.gs.is_some() { v73 |= 128; }
    r[73] = v73;

    let mut v74: u8 = 0;
    if ac.ias.is_some() { v74 |= 1; }
    if ac.tas.is_some() { v74 |= 2; }
    if ac.track.is_some() { v74 |= 8; }
    if ac.mag_heading.is_some() { v74 |= 64; }
    r[74] = v74;

    let mut v75: u8 = 0;
    if ac.baro_rate.is_some() { v75 |= 1; }
    if ac.geom_rate.is_some() { v75 |= 2; }
    r[75] = v75;

    let mut v76: u8 = 0;
    if ac.squawk.is_some() { v76 |= 4; }
    r[76] = v76;

    if let Some(ref cs) = ac.flight {
        for (i, b) in cs.as_bytes().iter().take(8).enumerate() { r[78 + i] = *b; }
    }

    if pos_fresh { write_i32(&mut r, 108, ((now_s - ac.seen_pos) * 10.0) as i32); }
    r
}

// === helpers ===

fn jstr(buf: &mut Vec<u8>, key: &str, val: &str) {
    buf.push(b'"'); buf.extend_from_slice(key.as_bytes());
    buf.extend_from_slice(b"\":\""); buf.extend_from_slice(val.as_bytes()); buf.push(b'"');
}
fn jint(buf: &mut Vec<u8>, key: &str, val: i32) {
    buf.push(b'"'); buf.extend_from_slice(key.as_bytes());
    buf.extend_from_slice(b"\":"); itoa(buf, val as i64);
}
fn jfloat(buf: &mut Vec<u8>, key: &str, val: f64, decimals: usize) {
    buf.push(b'"'); buf.extend_from_slice(key.as_bytes());
    buf.extend_from_slice(b"\":"); buf.extend_from_slice(format!("{:.1$}", val, decimals).as_bytes());
}
fn itoa(buf: &mut Vec<u8>, v: i64) { buf.extend_from_slice(v.to_string().as_bytes()); }

fn write_u32(buf: &mut [u8], off: usize, val: u32) { buf[off..off+4].copy_from_slice(&val.to_le_bytes()); }
fn write_i32(buf: &mut [u8], off: usize, val: i32) { buf[off..off+4].copy_from_slice(&val.to_le_bytes()); }
fn write_u16(buf: &mut [u8], off: usize, val: u16) { buf[off..off+2].copy_from_slice(&val.to_le_bytes()); }
fn write_i16(buf: &mut [u8], off: usize, val: i16) { buf[off..off+2].copy_from_slice(&val.to_le_bytes()); }
