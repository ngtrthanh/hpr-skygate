/// Aircraft state machine with CPR decode + speed check (no receiver location)

use std::collections::HashMap;
use std::collections::HashMap as StdHashMap;
use super::cpr;
use super::mode_s::ModeS;

const CPR_PAIR_TIMEOUT: f64 = 10.0;  // seconds between even/odd for global decode
const SPEED_MAX_KT: f64 = 900.0;     // max plausible speed for speed check
const STALE_TIMEOUT: f64 = 60.0;     // remove aircraft after 60s no message

fn now_s() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()
}

fn distance_nm(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    c * 3440.065 // earth radius in nm
}

#[derive(Debug, Clone)]
pub struct TracePoint {
    pub ts: f64,
    pub lat: f64,
    pub lon: f64,
    pub alt: Option<i32>,
    pub gs: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct CprSlot {
    pub even: Option<(u32, u32, f64)>, // (lat_cpr, lon_cpr, time)
    pub odd: Option<(u32, u32, f64)>,
}

pub struct Aircraft {
    pub hex: u32,
    pub flight: Option<String>,
    pub alt_baro: Option<i32>,
    pub alt_geom: Option<i32>,
    pub gs: Option<f64>,
    pub track: Option<f64>,
    pub baro_rate: Option<i32>,
    pub geom_rate: Option<i32>,
    pub squawk: Option<u16>,
    pub category: Option<u8>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub ias: Option<u16>,
    pub tas: Option<u16>,
    pub mag_heading: Option<f64>,
    pub nic: Option<u8>,
    pub nac_p: Option<u8>,
    pub nac_v: Option<u8>,
    pub sil: Option<u8>,
    pub gva: Option<u8>,
    pub sda: Option<u8>,
    pub nic_baro: Option<u8>,
    pub adsb_version: Option<u8>,
    pub nav_altitude_mcp: Option<u32>,
    pub nav_qnh: Option<f64>,
    pub nav_heading: Option<f64>,
    pub emergency: Option<u8>,
    pub messages: u64,
    pub seen: f64,           // last message timestamp
    pub seen_pos: f64,       // last position timestamp
    pub addr_type: u8,
    pub on_ground: bool,

    pub reg: Option<String>,      // registration from DB
    pub typecode: Option<String>, // type designator from DB
    pub route: Option<String>,    // learned route e.g. "VTBS-RJTT"
    pub trace: Vec<TracePoint>,

    // CPR state
    cpr_even: Option<(u32, u32, f64, u64)>, // (lat, lon, time, receiver_id)
    cpr_odd: Option<(u32, u32, f64, u64)>,
    /// Per-receiver CPR slots: key=receiver_id, value=(even, odd)
    cpr_slots: std::collections::HashMap<u64, CprSlot>,
    prev_lat: f64,
    prev_lon: f64,
    prev_pos_time: f64,
    pos_reliable_odd: f32,
    pos_reliable_even: f32,
}

impl Aircraft {
    fn new(icao: u32) -> Self {
        Self {
            hex: icao, flight: None, alt_baro: None, alt_geom: None,
            gs: None, track: None, baro_rate: None, geom_rate: None,
            squawk: None, category: None, lat: None, lon: None,
            ias: None, tas: None, mag_heading: None,
            nic: None, nac_p: None, nac_v: None, sil: None,
            gva: None, sda: None, nic_baro: None, adsb_version: None,
            nav_altitude_mcp: None, nav_qnh: None, nav_heading: None,
            emergency: None, messages: 0, seen: 0.0, seen_pos: 0.0,
            addr_type: 0, on_ground: false,
            reg: None, typecode: None, route: None,
            trace: Vec::new(),
            cpr_even: None, cpr_odd: None, cpr_slots: std::collections::HashMap::new(),
            prev_lat: 0.0, prev_lon: 0.0, prev_pos_time: 0.0,
            pos_reliable_odd: 0.0, pos_reliable_even: 0.0,
        }
    }
}

pub struct Store {
    pub map: HashMap<u32, Aircraft>,
    pub messages_total: u64,
    pub db: StdHashMap<u32, (String, String)>,
    pub enrichment: crate::enrichment::Enrichment,
    pub receiver_map: super::receiver_map::ReceiverMap,
}

impl Store {
    pub fn new(traffic_api: &str) -> Self {
        Self {
            map: HashMap::with_capacity(16384),
            messages_total: 0,
            db: load_aircraft_db(),
            enrichment: crate::enrichment::Enrichment::new(traffic_api),
            receiver_map: super::receiver_map::ReceiverMap::new(),
        }
    }

    pub fn update(&mut self, msg: ModeS) {
        if !msg.valid || msg.icao == 0 { return; }
        let t = now_s();
        self.messages_total += 1;

        // DF11 / Mode-S only: update existing, never create
        if msg.addr_type == 7 {
            if msg.df == 11 { return; } // DF11 = just ping, skip
            // DF4/5/20/21 carry altitude/squawk — create entry if has useful data
            let has_data = msg.altitude.is_some() || msg.squawk.is_some();
            if !has_data {
                return;
            }
            let ac = self.map.entry(msg.icao).or_insert_with(|| Aircraft::new(msg.icao));
            ac.messages += 1;
            ac.seen = t;
            ac.addr_type = 7;
            if let Some(alt) = msg.altitude { ac.alt_baro = Some(alt); }
            if let Some(sq) = msg.squawk { ac.squawk = Some(sq); }
            return;
        }

        // ADS-B (DF17/18): create or update
        let ac = self.map.entry(msg.icao).or_insert_with(|| Aircraft::new(msg.icao));
        if ac.reg.is_none() {
            if let Some((reg, typ)) = self.db.get(&msg.icao) {
                if !reg.is_empty() { ac.reg = Some(reg.clone()); }
                if !typ.is_empty() { ac.typecode = Some(typ.clone()); }
            }
        }
        ac.messages += 1;
        ac.seen = t;
        ac.addr_type = msg.addr_type;
        ac.on_ground = !msg.airborne;

        // Merge fields
        if let Some(alt) = msg.altitude { ac.alt_baro = Some(alt); }
        if let Some(alt) = msg.alt_gnss { ac.alt_geom = Some(alt); }
        if let Some(gs) = msg.gs { ac.gs = Some(gs); }
        if let Some(trk) = msg.track { ac.track = Some(trk); }
        if let Some(vr) = msg.baro_rate { ac.baro_rate = Some(vr); }
        if let Some(vr) = msg.geom_rate { ac.geom_rate = Some(vr); }
        if let Some(sq) = msg.squawk { ac.squawk = Some(sq); }
        if let Some(ref cs) = msg.callsign {
            ac.flight = Some(cs.clone());
            if ac.route.is_none() {
                if let Some(route) = self.enrichment.get_route(cs) {
                    ac.route = Some(route.to_string());
                }
            }
        }
        if let Some(cat) = msg.category { ac.category = Some(cat); }
        if let Some(v) = msg.ias { ac.ias = Some(v); }
        if let Some(v) = msg.tas { ac.tas = Some(v); }
        if let Some(v) = msg.mag_heading { ac.mag_heading = Some(v); }
        if let Some(v) = msg.nic { ac.nic = Some(v); }
        if let Some(v) = msg.nac_p { ac.nac_p = Some(v); }
        if let Some(v) = msg.nac_v { ac.nac_v = Some(v); }
        if let Some(v) = msg.sil { ac.sil = Some(v); }
        if let Some(v) = msg.gva { ac.gva = Some(v); }
        if let Some(v) = msg.sda { ac.sda = Some(v); }
        if let Some(v) = msg.nic_baro { ac.nic_baro = Some(v); }
        if let Some(v) = msg.adsb_version { ac.adsb_version = Some(v); }
        if let Some(v) = msg.nav_altitude_mcp { ac.nav_altitude_mcp = Some(v); }
        if let Some(v) = msg.nav_qnh { ac.nav_qnh = Some(v); }
        if let Some(v) = msg.nav_heading { ac.nav_heading = Some(v); }
        if let Some(v) = msg.emergency { ac.emergency = Some(v); }

        // CPR position decode — store per receiver
        if let (Some(cpr_lat), Some(cpr_lon), Some(odd)) = (msg.cpr_lat, msg.cpr_lon, msg.cpr_odd) {
            let rid = msg.receiver_id;
            {
                let slot = ac.cpr_slots.entry(rid).or_insert(CprSlot { even: None, odd: None });
                if odd { slot.odd = Some((cpr_lat, cpr_lon, t)); }
                else { slot.even = Some((cpr_lat, cpr_lon, t)); }
            }
            // Also store in legacy fields for relative decode (uses any recent frame)
            if odd {
                ac.cpr_odd = Some((cpr_lat, cpr_lon, t, rid));
            } else {
                ac.cpr_even = Some((cpr_lat, cpr_lon, t, rid));
            }
            let prev_seen = ac.seen_pos;
            self.try_position(msg.icao, t);
            // Validate against receiver coverage map — revoke if out of range
            if let Some(ac) = self.map.get_mut(&msg.icao) {
                if ac.seen_pos > prev_seen {
                    if let (Some(lat), Some(lon)) = (ac.lat, ac.lon) {
                        if self.receiver_map.check_position(msg.receiver_id, lat, lon) {
                            // Confirmed — feed to receiver map for learning
                            self.receiver_map.position_received(msg.receiver_id, lat, lon);
                        } else {
                            // Out of receiver range — revoke position, reset
                            ac.lat = None;
                            ac.lon = None;
                            ac.prev_pos_time = 0.0;
                            ac.prev_lat = 0.0;
                            ac.prev_lon = 0.0;
                            ac.pos_reliable_odd = 0.0;
                            ac.pos_reliable_even = 0.0;
                        }
                    }
                }
            }
        }
    }

    fn try_position(&mut self, icao: u32, t: f64) {
        let ac = match self.map.get_mut(&icao) { Some(a) => a, None => return };

        // Try relative decode first if we have an internal position
        if ac.prev_pos_time > 0.0 {
            let (cpr_lat, cpr_lon, is_odd) = match (ac.cpr_even, ac.cpr_odd) {
                (Some(e), Some(o)) => {
                    if e.2 > o.2 && e.3 != 0 { (e.0, e.1, false) }
                    else if o.3 != 0 { (o.0, o.1, true) }
                    else if e.3 != 0 { (e.0, e.1, false) }
                    else { return; }
                }
                (Some(e), None) => if e.3 != 0 { (e.0, e.1, false) } else { return; },
                (None, Some(o)) => if o.3 != 0 { (o.0, o.1, true) } else { return; },
                _ => return,
            };
            if let Some((lat, lon)) = cpr::decode_cpr_relative(
                ac.prev_lat, ac.prev_lon, cpr_lat, cpr_lon, is_odd, ac.on_ground
            ) {
                if speed_check(ac, lat, lon, t) {
                    ac.prev_lat = lat;
                    ac.prev_lon = lon;
                    ac.prev_pos_time = t;
                    // Increment reliability on consistent relative decode
                    if is_odd { ac.pos_reliable_odd += 1.0; } else { ac.pos_reliable_even += 1.0; }
                    // Publish if reliable
                    if ac.pos_reliable_odd >= 2.0 && ac.pos_reliable_even >= 2.0 {
                        ac.lat = Some(lat);
                        ac.lon = Some(lon);
                        ac.seen_pos = t;
                        ac.trace.push(TracePoint { ts: t, lat, lon, alt: ac.alt_baro, gs: ac.gs });
                        if ac.trace.len() > 1000 { ac.trace.remove(0); }
                    }
                    return;
                }
            }
            return;
        }

        // Global decode: find a per-receiver slot with both even+odd within timeout
        let mut best: Option<(u32, u32, u32, u32, bool, u64)> = None;
        for (&rid, slot) in &ac.cpr_slots {
            if let (Some(e), Some(o)) = (slot.even, slot.odd) {
                if (e.2 - o.2).abs() <= CPR_PAIR_TIMEOUT {
                    let fflag = o.2 > e.2;
                    best = Some((e.0, e.1, o.0, o.1, fflag, rid));
                    break;
                }
            }
        }
        let (elat, elon, olat, olon, fflag, rid) = match best {
            Some(p) => p,
            None => return,
        };
        if let Some((lat, lon)) = cpr::decode_cpr_airborne(elat, elon, olat, olon, fflag) {
            if !speed_check(ac, lat, lon, t) {
                // Speed check failed — decrement reliability
                if fflag { ac.pos_reliable_odd -= 1.0; } else { ac.pos_reliable_even -= 1.0; }
                if ac.pos_reliable_odd < 0.0 || ac.pos_reliable_even < 0.0 {
                    ac.prev_pos_time = 0.0; ac.prev_lat = 0.0; ac.prev_lon = 0.0;
                    ac.pos_reliable_odd = 0.0; ac.pos_reliable_even = 0.0;
                    ac.lat = None; ac.lon = None;
                }
                return;
            }

            // Fast-track: if within 50km of last reliable position, trust immediately
            if ac.lat.is_some() {
                let dist = distance_nm(ac.lat.unwrap(), ac.lon.unwrap(), lat, lon);
                if dist < 27.0 { // 50km ≈ 27nm
                    ac.pos_reliable_odd = 2.0_f32.max(ac.pos_reliable_odd);
                    ac.pos_reliable_even = 2.0_f32.max(ac.pos_reliable_even);
                }
            }

            // Store internal position
            ac.prev_lat = lat;
            ac.prev_lon = lon;
            ac.prev_pos_time = t;

            // Increment reliability counter (both — global uses both even+odd)
            ac.pos_reliable_odd += 1.0;
            ac.pos_reliable_even += 1.0;

            // Publish only when reliable (both counters ≥ 2)
            if ac.pos_reliable_odd >= 2.0 && ac.pos_reliable_even >= 2.0 {
                ac.lat = Some(lat);
                ac.lon = Some(lon);
                ac.seen_pos = t;
                ac.trace.push(TracePoint { ts: t, lat, lon, alt: ac.alt_baro, gs: ac.gs });
                if ac.trace.len() > 1000 { ac.trace.remove(0); }
            }
        }
    }
    pub fn reap_stale(&mut self) {
        let t = now_s();
        self.map.retain(|_, ac| t - ac.seen < STALE_TIMEOUT);
        self.enrichment.tick();
        self.receiver_map.persist();
    }

    pub fn aircraft_count(&self) -> usize { self.map.len() }
    pub fn with_position_count(&self) -> usize { self.map.values().filter(|a| a.lat.is_some()).count() }
}

fn load_aircraft_db() -> StdHashMap<u32, (String, String)> {
    let path = "/opt/workspace/dev/hpradar.com/skylink/output/skylink-core/aircrafts.json";
    let mut map = StdHashMap::new();
    if let Ok(data) = std::fs::read_to_string(path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
            if let Some(obj) = v.as_object() {
                for (hex, info) in obj {
                    if let Ok(icao) = u32::from_str_radix(hex, 16) {
                        let (reg, typ) = if let Some(arr) = info.as_array() {
                            // Format: [registration, type_designator, ...]
                            let r = arr.get(0).and_then(|v| v.as_str()).unwrap_or("");
                            let t = arr.get(1).and_then(|v| v.as_str()).unwrap_or("");
                            (r.to_string(), t.to_string())
                        } else {
                            let r = info.get("r").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let t = info.get("t").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            (r, t)
                        };
                        if !reg.is_empty() || !typ.is_empty() {
                            map.insert(icao, (reg, typ));
                        }
                    }
                }
            }
        }
    }
    tracing::info!(count = map.len(), "aircraft db loaded");
    map
}

fn speed_check(ac: &Aircraft, lat: f64, lon: f64, t: f64) -> bool {
    // Reject impossible positions
    if lat.abs() > 82.0 { return false; }

    if ac.prev_pos_time == 0.0 { return true; }
    let elapsed = t - ac.prev_pos_time;
    if elapsed < 0.1 { return true; }
    let dist = distance_nm(ac.prev_lat, ac.prev_lon, lat, lon);
    let speed = dist / (elapsed / 3600.0);
    if speed >= SPEED_MAX_KT { return false; }

    // Track direction check: if we have heading and moved > 5nm, reject if direction > 90° off
    if dist > 5.0 {
        if let Some(track) = ac.track {
            let bearing = bearing_deg(ac.prev_lat, ac.prev_lon, lat, lon);
            let diff = (bearing - track + 540.0) % 360.0 - 180.0;
            // Allow 120° deviation (aircraft can turn, but not reverse)
            if diff.abs() > 120.0 && elapsed < 30.0 {
                return false;
            }
        }
    }
    true
}

fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let lat1 = lat1.to_radians();
    let lat2 = lat2.to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let x = dlon.sin() * lat2.cos();
    let y = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    (x.atan2(y).to_degrees() + 360.0) % 360.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_df11_does_not_create() {
        let mut store = Store::new("");
        let msg = ModeS { df: 11, icao: 0xABCDEF, addr_type: 7, valid: true, ..default_msg() };
        store.update(msg);
        assert_eq!(store.aircraft_count(), 0);
    }

    #[test]
    fn test_df17_creates_aircraft() {
        let mut store = Store::new("");
        let msg = ModeS { df: 17, icao: 0x4840D6, addr_type: 0, valid: true,
            callsign: Some("KLM1023".into()), ..default_msg() };
        store.update(msg);
        assert_eq!(store.aircraft_count(), 1);
        assert_eq!(store.map.get(&0x4840D6).unwrap().flight.as_deref(), Some("KLM1023"));
    }

    #[test]
    fn test_speed_check_rejects_teleport() {
        let ac = Aircraft {
            prev_lat: 13.7, prev_lon: 100.5, prev_pos_time: 1000.0,
            ..Aircraft::new(0x123456)
        };
        let t = 1005.0; // 5s later
        // Bangkok to Amsterdam in 5s = impossible
        assert!(!speed_check(&ac, 52.0, 4.0, t));
        // Bangkok to nearby in 5s = fine (0.1 deg ≈ 6nm, 6nm/5s = 4320kt... too fast)
        // Use smaller move: 0.01 deg ≈ 0.6nm in 5s = 432kt, ok
        assert!(speed_check(&ac, 13.71, 100.51, t));
    }

    fn default_msg() -> ModeS {
        ModeS {
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
