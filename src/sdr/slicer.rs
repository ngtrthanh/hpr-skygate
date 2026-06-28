pub struct SliceResult {
    pub frame: [u8; 14],
    pub confidence: [u8; 112],
    pub bits: usize,
}

pub fn slice_manchester(mag: &[u16], offset: usize) -> SliceResult {
    let mut frame = [0u8; 14];
    let mut confidence = [0u8; 112];
    let base = offset + 16;

    for bit in 0..112 {
        let s0 = mag[base + bit * 2] as i32;
        let s1 = mag[base + bit * 2 + 1] as i32;
        let delta = s0 - s1;

        if delta > 0 {
            frame[bit / 8] |= 1 << (7 - (bit % 8));
        }

        let sum = (s0 + s1).max(1);
        let conf = ((delta.unsigned_abs() * 255) / sum as u32).min(255) as u8;
        confidence[bit] = conf;
    }

    let df = frame[0] >> 3;
    let bits = if df >= 16 { 112 } else { 56 };

    SliceResult { frame, confidence, bits }
}
