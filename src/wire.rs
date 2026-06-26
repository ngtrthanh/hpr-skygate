/// HPRadar §8 binary frame format for aviation
/// Position frame: type=0x01, 19 bytes
/// Static frame: type=0x05, 44 bytes

use crate::decode::aircraft::Aircraft;

pub fn encode_position(ac: &Aircraft) -> Option<[u8; 19]> {
    let (lat, lon) = match (ac.lat, ac.lon) {
        (Some(la), Some(lo)) => (la, lo),
        _ => return None,
    };
    let mut buf = [0u8; 19];
    buf[0] = 0x01;
    buf[1] = (ac.hex >> 16) as u8;
    buf[2] = (ac.hex >> 8) as u8;
    buf[3] = ac.hex as u8;
    let lon_i = (lon * 600000.0) as i32;
    let lat_i = (lat * 600000.0) as i32;
    buf[4..8].copy_from_slice(&lon_i.to_le_bytes());
    buf[8..12].copy_from_slice(&lat_i.to_le_bytes());
    let gs = ac.gs.unwrap_or(0.0);
    let trk = ac.track.unwrap_or(0.0);
    let alt = ac.alt_baro.unwrap_or(0) as i16;
    buf[12..14].copy_from_slice(&((gs * 10.0) as u16).to_le_bytes());
    buf[14..16].copy_from_slice(&((trk * 10.0) as u16).to_le_bytes());
    buf[16..18].copy_from_slice(&alt.to_le_bytes());
    let mut flags = 0u8;
    if ac.on_ground { flags |= 1; }
    if ac.alt_baro.is_some() { flags |= 2; }
    buf[18] = flags;
    Some(buf)
}

pub fn encode_static(ac: &Aircraft) -> Option<[u8; 44]> {
    if ac.flight.is_none() && ac.typecode.is_none() { return None; }
    let mut buf = [0u8; 44];
    buf[0] = 0x05;
    buf[1] = (ac.hex >> 16) as u8;
    buf[2] = (ac.hex >> 8) as u8;
    buf[3] = ac.hex as u8;
    if let Some(ref cs) = ac.flight {
        for (i, b) in cs.as_bytes().iter().take(8).enumerate() { buf[4 + i] = *b; }
    }
    buf[12] = ac.category.unwrap_or(0);
    if let Some(sq) = ac.squawk {
        let s = format!("{:04}", sq);
        for (i, b) in s.as_bytes().iter().take(4).enumerate() { buf[13 + i] = *b; }
    }
    if let Some(ref t) = ac.typecode {
        for (i, b) in t.as_bytes().iter().take(4).enumerate() { buf[17 + i] = *b; }
    }
    if let Some(ref r) = ac.reg {
        for (i, b) in r.as_bytes().iter().take(12).enumerate() { buf[21 + i] = *b; }
    }
    if let Some(ref route) = ac.route {
        for (i, b) in route.as_bytes().iter().take(11).enumerate() { buf[33 + i] = *b; }
    }
    Some(buf)
}
