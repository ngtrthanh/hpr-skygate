/// 1090 MHz Mode-S IQ demodulator (from hpr-demod)
/// Processes raw IQ samples → validated Mode-S frames

use super::mag::{iq_to_magnitude, MagLut};
use super::preamble::detect_preamble;
use super::slicer::slice_manchester;
use crate::decode::crc::{modes_checksum, modes_fix_single_bit};

pub struct Demod1090 {
    lut: MagLut,
    mag_buf: Vec<u16>,
    pub frames_ok: u64,
    pub frames_fixed: u64,
    pub frames_rejected: u64,
}

impl Demod1090 {
    pub fn new() -> Self {
        Self {
            lut: MagLut::new(),
            mag_buf: vec![0u16; 128 * 1024],
            frames_ok: 0,
            frames_fixed: 0,
            frames_rejected: 0,
        }
    }

    /// Process a chunk of raw IQ data. Calls `emit` for each valid frame (payload bytes, signal).
    pub fn process_chunk(&mut self, iq: &[u8], mut emit: impl FnMut(&[u8], u8)) {
        let samples = iq.len() / 2;
        if self.mag_buf.len() < samples {
            self.mag_buf.resize(samples, 0);
        }
        iq_to_magnitude(&self.lut, iq, &mut self.mag_buf[..samples]);
        self.scan_frames(samples, &mut emit);
    }

    fn scan_frames(&mut self, samples: usize, emit: &mut impl FnMut(&[u8], u8)) {
        let mut offset = 0;
        while offset + 16 + 224 <= samples {
            if detect_preamble(&self.mag_buf[..samples], offset) {
                let mut result = slice_manchester(&self.mag_buf[..samples], offset);
                let signal = ((self.mag_buf[offset] as u32 + self.mag_buf[offset + 2] as u32
                    + self.mag_buf[offset + 7] as u32 + self.mag_buf[offset + 9] as u32)
                    / 4 / 256).min(255) as u8;

                if self.try_validate(&mut result.frame, result.bits, &result.confidence) {
                    emit(&result.frame[..result.bits / 8], signal);
                    offset += 16 + result.bits * 2;
                    continue;
                }
                self.frames_rejected += 1;
            }
            offset += 1;
        }
    }

    fn try_validate(&mut self, frame: &mut [u8; 14], bits: usize, confidence: &[u8; 112]) -> bool {
        let avg_conf: u32 = confidence[..bits].iter().map(|&c| c as u32).sum::<u32>() / bits as u32;
        if avg_conf < 40 { return false; }

        let crc = modes_checksum(&frame[..bits / 8]);
        let df = frame[0] >> 3;

        if df == 11 || df == 17 || df == 18 {
            if crc == 0 {
                self.frames_ok += 1;
                return true;
            }
            if df == 17 || df == 18 {
                if modes_fix_single_bit(&mut frame[..bits / 8]) {
                    self.frames_fixed += 1;
                    return true;
                }
            }
            return false;
        }
        match df {
            0 | 4 | 5 | 16 | 20 | 21 => {
                if avg_conf < 80 { return false; }
                let icao = crc & 0xFFFFFF;
                if icao != 0 && icao != 0xFFFFFF {
                    self.frames_ok += 1;
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}
