use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Vessel {
    pub mmsi: u32,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub sog: Option<f64>,
    pub cog: Option<f64>,
    pub hdg: Option<u16>,
    pub name: Option<String>,
    pub callsign: Option<String>,
    pub ship_type: Option<u8>,
    pub imo: Option<u32>,
    pub seen: f64,
}

pub struct VesselStore {
    pub map: HashMap<u32, Vessel>,
}

impl VesselStore {
    pub fn new() -> Self {
        Self { map: HashMap::with_capacity(8192) }
    }

    fn now_ts() -> f64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
    }

    /// Update vessel from a position frame (0x01)
    pub fn update_position(&mut self, data: &[u8; 19]) {
        let mmsi = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
        let lon = i32::from_le_bytes([data[5], data[6], data[7], data[8]]) as f64 / 600000.0;
        let lat = i32::from_le_bytes([data[9], data[10], data[11], data[12]]) as f64 / 600000.0;
        let sog = u16::from_le_bytes([data[13], data[14]]) as f64 / 10.0;
        let cog = u16::from_le_bytes([data[15], data[16]]) as f64 / 10.0;
        let hdg = u16::from_le_bytes([data[17], data[18]]);
        let ts = Self::now_ts();

        let v = self.map.entry(mmsi).or_insert_with(|| Vessel {
            mmsi, lat: None, lon: None, sog: None, cog: None, hdg: None,
            name: None, callsign: None, ship_type: None, imo: None, seen: ts,
        });
        v.lat = Some(lat);
        v.lon = Some(lon);
        v.sog = Some(sog);
        v.cog = Some(cog);
        v.hdg = if hdg == 511 { None } else { Some(hdg) };
        v.seen = ts;
    }

    /// Update vessel from a static frame (0x05)
    pub fn update_static(&mut self, data: &[u8; 44], name: &str, callsign: &str, ship_type: u8) {
        let mmsi = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
        let imo = u32::from_le_bytes([data[33], data[34], data[35], data[36]]);
        let ts = Self::now_ts();

        let v = self.map.entry(mmsi).or_insert_with(|| Vessel {
            mmsi, lat: None, lon: None, sog: None, cog: None, hdg: None,
            name: None, callsign: None, ship_type: None, imo: None, seen: ts,
        });
        if !name.is_empty() { v.name = Some(name.to_string()); }
        if !callsign.is_empty() { v.callsign = Some(callsign.to_string()); }
        if ship_type != 0 { v.ship_type = Some(ship_type); }
        if imo != 0 { v.imo = Some(imo); }
        v.seen = ts;
    }

    /// Remove stale vessels (not seen in 30 minutes)
    pub fn reap_stale(&mut self) {
        let cutoff = Self::now_ts() - 1800.0;
        self.map.retain(|_, v| v.seen > cutoff);
    }

    pub fn count(&self) -> usize { self.map.len() }

    /// Build JSON for /api/vessels
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(&self.map.values().collect::<Vec<_>>()).unwrap_or_default()
    }
}
