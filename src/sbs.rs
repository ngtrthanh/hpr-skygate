use crate::decode::aircraft::Aircraft;
use crate::decode::mode_s::ModeS;

pub fn format_sbs(msg: &ModeS, ac: &Aircraft) -> Option<Vec<u8>> {
    let now = chrono_now();
    let hex = format!("{:06X}", msg.icao);
    let line = if let (Some(lat), Some(lon)) = (ac.lat, ac.lon) {
        if ac.alt_baro.is_some() {
            format!("MSG,3,1,1,{hex},1,{now},{now},{now},{now},{alt},{gs},{trk},{lat},{lon},,,{vr},,,,0\r\n",
                hex=hex, now=now,
                alt=ac.alt_baro.unwrap_or(0),
                gs=ac.gs.map(|v| format!("{:.1}", v)).unwrap_or_default(),
                trk=ac.track.map(|v| format!("{:.1}", v)).unwrap_or_default(),
                lat=lat, lon=lon,
                vr=ac.baro_rate.unwrap_or(0))
        } else { return None; }
    } else if let Some(ref cs) = ac.flight {
        format!("MSG,1,1,1,{hex},1,{now},{now},{now},{now},{cs},,,,,,,,,,\r\n",
            hex=hex, now=now, cs=cs)
    } else { return None; };
    Some(line.into_bytes())
}

fn chrono_now() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let secs = d.as_secs();
    let ms = d.subsec_millis();
    let days = secs / 86400; let rem = secs % 86400;
    let h = rem / 3600; let m = (rem % 3600) / 60; let s = rem % 60;
    let (y, mo, day) = epoch_days_to_date(days);
    format!("{:04}/{:02}/{:02},{:02}:{:02}:{:02}.{:03}", y, mo, day, h, m, s, ms)
}

fn epoch_days_to_date(days: u64) -> (u64, u64, u64) {
    let y = 1970 + days / 365;
    let d = days % 365;
    let m = d / 30 + 1;
    let day = d % 30 + 1;
    (y, m.min(12), day.min(28))
}
