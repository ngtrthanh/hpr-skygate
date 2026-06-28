#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

pub struct MagLut {
    pub table: Vec<u16>,
}

impl MagLut {
    pub fn new() -> Self {
        let mut table = vec![0u16; 65536];
        for i in 0..=255u16 {
            for q in 0..=255u16 {
                let fi = i as f64 - 127.5;
                let fq = q as f64 - 127.5;
                let mag = (fi * fi + fq * fq).sqrt();
                table[((i as usize) << 8) | q as usize] = (mag * 181.0) as u16;
            }
        }
        Self { table }
    }

    #[inline(always)]
    pub fn lookup(&self, i: u8, q: u8) -> u16 {
        unsafe { *self.table.get_unchecked(((i as usize) << 8) | q as usize) }
    }
}

/// LUT magnitude conversion with 4x loop unrolling.
#[inline(always)]
pub fn iq_to_magnitude(lut: &MagLut, iq: &[u8], mag: &mut [u16]) {
    let n = mag.len();
    let chunks = n / 4;
    for c in 0..chunks {
        let base = c * 4;
        let iq_base = base * 2;
        unsafe {
            *mag.get_unchecked_mut(base) = *lut.table.get_unchecked(((iq[iq_base] as usize) << 8) | iq[iq_base + 1] as usize);
            *mag.get_unchecked_mut(base + 1) = *lut.table.get_unchecked(((iq[iq_base + 2] as usize) << 8) | iq[iq_base + 3] as usize);
            *mag.get_unchecked_mut(base + 2) = *lut.table.get_unchecked(((iq[iq_base + 4] as usize) << 8) | iq[iq_base + 5] as usize);
            *mag.get_unchecked_mut(base + 3) = *lut.table.get_unchecked(((iq[iq_base + 6] as usize) << 8) | iq[iq_base + 7] as usize);
        }
    }
    for i in (chunks * 4)..n {
        mag[i] = lut.lookup(iq[i * 2], iq[i * 2 + 1]);
    }
}

/// SIMD magnitude approximation (no LUT). Uses mag ≈ max(|I-128|,|Q-128|) + min/4.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn iq_to_magnitude_simd(iq: &[u8], mag: &mut [u16]) {
    let n = iq.len() / 2;
    let center = _mm256_set1_epi8(127u8 as i8);
    let chunks = n / 16;

    for c in 0..chunks {
        let raw = _mm256_loadu_si256(iq.as_ptr().add(c * 32) as *const __m256i);
        let signed = _mm256_sub_epi8(raw, center);
        let abs_val = _mm256_abs_epi8(signed);
        // Deinterleave: I = even bytes (mask low byte of each 16-bit word), Q = odd bytes (shift right)
        let i_vals = _mm256_and_si256(abs_val, _mm256_set1_epi16(0x00FF));
        let q_vals = _mm256_srli_epi16::<8>(abs_val);
        let mx = _mm256_max_epi16(i_vals, q_vals);
        let mn = _mm256_min_epi16(i_vals, q_vals);
        let approx = _mm256_add_epi16(mx, _mm256_srli_epi16::<2>(mn));
        _mm256_storeu_si256(mag.as_mut_ptr().add(c * 16) as *mut __m256i, approx);
    }
    // Remainder
    for i in (chunks * 16)..n {
        let ii = (iq[i * 2] as i16 - 128).unsigned_abs() as u16;
        let qq = (iq[i * 2 + 1] as i16 - 128).unsigned_abs() as u16;
        let mx = ii.max(qq);
        let mn = ii.min(qq);
        *mag.get_unchecked_mut(i) = mx + (mn >> 2);
    }
}
