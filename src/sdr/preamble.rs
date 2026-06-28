#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

pub fn detect_preamble(mag: &[u16], offset: usize) -> bool {
    if offset + 16 + 112 * 2 > mag.len() {
        return false;
    }
    let m = &mag[offset..];

    if m[0] <= m[1] || m[2] <= m[3] || m[7] <= m[8] || m[9] <= m[10] {
        return false;
    }

    let p0 = m[0] as u32;
    let p1 = m[2] as u32;
    let p2 = m[7] as u32;
    let p3 = m[9] as u32;
    let pulse_sum = p0 + p1 + p2 + p3;

    let valley_sum = m[1] as u32 + m[3] as u32 + m[4] as u32 + m[5] as u32
        + m[6] as u32 + m[8] as u32 + m[10] as u32 + m[11] as u32
        + m[12] as u32 + m[13] as u32 + m[14] as u32 + m[15] as u32;

    if pulse_sum * 3 <= valley_sum * 2 {
        return false;
    }

    if p0.min(p1).min(p2).min(p3) < 50 {
        return false;
    }

    true
}

/// SIMD pre-filter: find sample offsets where mag > threshold (potential preamble starts).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn scan_candidates_simd(mag: &[u16], threshold: u16, candidates: &mut Vec<usize>) {
    candidates.clear();
    let thresh = _mm256_set1_epi16(threshold as i16);
    let n = mag.len().saturating_sub(240); // need room for full frame
    let chunks = n / 16;

    for c in 0..chunks {
        let v = _mm256_loadu_si256(mag.as_ptr().add(c * 16) as *const __m256i);
        let cmp = _mm256_cmpgt_epi16(v, thresh);
        let mask = _mm256_movemask_epi8(cmp) as u32;
        if mask != 0 {
            let base = c * 16;
            for bit in 0..16u32 {
                if mask & (3 << (bit * 2)) != 0 {
                    candidates.push(base + bit as usize);
                }
            }
        }
    }
}

/// Scalar fallback candidate scanner.
pub fn scan_candidates_scalar(mag: &[u16], threshold: u16, candidates: &mut Vec<usize>) {
    candidates.clear();
    let n = mag.len().saturating_sub(240);
    for i in 0..n {
        if mag[i] > threshold {
            candidates.push(i);
        }
    }
}
