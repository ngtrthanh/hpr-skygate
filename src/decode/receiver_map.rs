/// Self-learning receiver coverage map.
/// Learns each receiver's geographic extent from confirmed positions.
/// Rejects positions far outside a receiver's known coverage area.

use std::collections::HashMap;

const MIN_POSITIONS: u32 = 50;       // need this many before enforcing range
const MAX_RANGE_KM: f64 = 600.0;     // max plausible receiver range
const GROWTH_MARGIN_KM: f64 = 50.0;  // allow extent to grow by this much per update

pub struct ReceiverMap {
    receivers: HashMap<u64, ReceiverExtent>,
}

struct ReceiverExtent {
    lat_min: f64,
    lat_max: f64,
    lon_min: f64,
    lon_max: f64,
    positions: u32,
}

impl ReceiverExtent {
    fn center(&self) -> (f64, f64) {
        ((self.lat_min + self.lat_max) / 2.0, (self.lon_min + self.lon_max) / 2.0)
    }

    fn range_km(&self) -> f64 {
        let dlat = (self.lat_max - self.lat_min) * 111.0;
        let dlon = (self.lon_max - self.lon_min) * 111.0 * ((self.lat_min + self.lat_max) / 2.0).to_radians().cos();
        (dlat * dlat + dlon * dlon).sqrt() / 2.0
    }
}

impl ReceiverMap {
    pub fn new() -> Self {
        Self { receivers: HashMap::with_capacity(4096) }
    }

    /// Record a confirmed position for a receiver. Grows the receiver's extent.
    pub fn position_received(&mut self, receiver_id: u64, lat: f64, lon: f64) {
        if receiver_id == 0 || lat.abs() > 85.0 || lon.abs() > 180.0 { return; }

        let r = self.receivers.entry(receiver_id).or_insert(ReceiverExtent {
            lat_min: lat, lat_max: lat, lon_min: lon, lon_max: lon, positions: 0,
        });

        // Don't grow too fast — limit growth per update
        let margin = GROWTH_MARGIN_KM / 111.0; // degrees
        if lat < r.lat_min && (r.lat_min - lat) < margin { r.lat_min = lat; }
        if lat > r.lat_max && (lat - r.lat_max) < margin { r.lat_max = lat; }
        if lon < r.lon_min && (r.lon_min - lon) < margin { r.lon_min = lon; }
        if lon > r.lon_max && (lon - r.lon_max) < margin { r.lon_max = lon; }

        r.positions = r.positions.saturating_add(1);
    }

    /// Check if a position is plausible for a given receiver.
    /// Returns true if:
    /// - receiver unknown (not enough data yet) → allow
    /// - position within receiver's learned extent + margin → allow
    /// - position outside → reject
    pub fn check_position(&self, receiver_id: u64, lat: f64, lon: f64) -> bool {
        if receiver_id == 0 { return true; } // can't validate unknown receivers

        let r = match self.receivers.get(&receiver_id) {
            Some(r) => r,
            None => return true, // new receiver, allow
        };

        // Don't enforce until we have enough data
        if r.positions < MIN_POSITIONS { return true; }

        let (clat, clon) = r.center();
        let dist_km = haversine_km(clat, clon, lat, lon);
        let extent_km = r.range_km();

        // Allow if within extent + MAX_RANGE_KM buffer
        dist_km < extent_km + MAX_RANGE_KM
    }

    pub fn receiver_count(&self) -> usize { self.receivers.len() }
    pub fn learned_count(&self) -> usize {
        self.receivers.values().filter(|r| r.positions >= MIN_POSITIONS).count()
    }
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    6371.0 * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
}
