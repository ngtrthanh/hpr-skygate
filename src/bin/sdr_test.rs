/// SDR demod test — uses rtl_sdr CLI for IQ capture, Rust for demodulation
/// Usage: sdr-test [--ais SERIAL] [--adsb SERIAL] [--duration SECS]
///
/// Shells out to `rtl_sdr` (must be in PATH) for IQ capture,
/// then demodulates 1090 MHz (Mode-S) or 162 MHz (AIS) in pure Rust.

use std::collections::HashSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ===== Magnitude LUT =====
struct MagLut([u16; 65536]);
impl MagLut {
    fn new() -> Self {
        let mut lut = [0u16; 65536];
        for i in 0..256u32 {
            for q in 0..256u32 {
                let iv = i as f64 - 127.5;
                let qv = q as f64 - 127.5;
                lut[(i * 256 + q) as usize] = (iv * iv + qv * qv).sqrt() as u16;
            }
        }
        Self(lut)
    }
    fn convert(&self, iq: &[u8], mag: &mut [u16]) {
        for i in 0..mag.len() {
            mag[i] = self.0[(iq[i*2] as usize) * 256 + iq[i*2+1] as usize];
        }
    }
}

// ===== 1090 MHz Mode-S Demod =====
fn detect_preamble(mag: &[u16], off: usize) -> bool {
    if off + 16 + 224 > mag.len() { return false; }
    // Preamble: 1010000101000000 (positions 0,2,7,9 are high)
    let high = (mag[off] as u32 + mag[off+2] as u32 + mag[off+7] as u32 + mag[off+9] as u32) / 4;
    let low = (mag[off+1] as u32 + mag[off+3] as u32 + mag[off+4] as u32 + mag[off+5] as u32
        + mag[off+6] as u32 + mag[off+8] as u32) / 6;
    high > 2 * low && high > 10
}

fn demod_1090_chunk(iq: &[u8], lut: &MagLut, mag: &mut Vec<u16>) -> Vec<(u8, [u8;14], usize)> {
    let samples = iq.len() / 2;
    if mag.len() < samples { mag.resize(samples, 0); }
    lut.convert(iq, &mut mag[..samples]);

    let mut frames = Vec::new();
    let mut off = 0;
    while off + 16 + 224 <= samples {
        if detect_preamble(mag, off) {
            let data_start = off + 16;
            let mut frame = [0u8; 14];
            let mut conf_sum: u32 = 0;

            // Slice 112 bits (Manchester)
            for bit in 0..112 {
                let a = mag[data_start + bit*2] as i32;
                let b = mag[data_start + bit*2 + 1] as i32;
                let diff = a - b;
                conf_sum += diff.unsigned_abs();
                if diff > 0 { frame[bit/8] |= 1 << (7 - (bit % 8)); }
            }

            let df = frame[0] >> 3;
            let bits = if df >= 16 { 112 } else { 56 };
            let avg_conf = conf_sum / bits as u32;
            if avg_conf < 3 { off += 1; continue; }

            let crc = modes_crc(&frame[..bits/8]);

            let valid = match df {
                11 | 17 | 18 => crc == 0,
                0 | 4 | 5 | 16 | 20 | 21 => avg_conf > 8 && crc != 0 && crc != 0xFFFFFF,
                _ => false,
            };

            if valid {
                // Try 1-bit fix for DF17/18
                let fixed = if (df == 17 || df == 18) && crc != 0 {
                    fix_single_bit(&mut frame, bits)
                } else { crc == 0 };

                if fixed || (df != 17 && df != 18) {
                    frames.push((df, frame, bits));
                    off += 16 + bits * 2;
                    continue;
                }
            }
        }
        off += 1;
    }
    frames
}

const CRC_POLY: u32 = 0xFFF409;

fn modes_crc(msg: &[u8]) -> u32 {
    let n = msg.len();
    if n < 3 { return 0xFFFFFF; }
    let mut crc: u32 = 0;
    for i in 0..n-3 {
        let byte = msg[i] as u32;
        for bit in (0..8).rev() {
            if (crc ^ (byte << (bit + 16))) & 0x800000 != 0 {
                crc = (crc << 1) ^ CRC_POLY;
            } else {
                crc <<= 1;
            }
            crc &= 0xFFFFFF;
        }
    }
    crc ^= ((msg[n-3] as u32) << 16) | ((msg[n-2] as u32) << 8) | msg[n-1] as u32;
    crc & 0xFFFFFF
}

fn fix_single_bit(msg: &mut [u8; 14], bits: usize) -> bool {
    let orig_crc = modes_crc(&msg[..bits/8]);
    if orig_crc == 0 { return true; }
    for bit in 0..bits {
        msg[bit/8] ^= 1 << (7 - (bit % 8));
        if modes_crc(&msg[..bits/8]) == 0 { return true; }
        msg[bit/8] ^= 1 << (7 - (bit % 8));
    }
    false
}

// ===== 162 MHz AIS GMSK Demod =====
// Architecture from ais-catcher ModelStandard:
// 288kSPS → Rotate ±25kHz (channel split) → decimate ÷6 → 48kSPS
// → FM discriminator → 37-tap matched filter → Deinterleave(5) → 5× HDLC decoder

const SAMPLES_PER_BIT: usize = 5; // 48000 / 9600

// Matched filter from ais-catcher (Filters::Receiver, 37 taps)
const FILTER_TAPS: [f32; 37] = [
    0.00119025, -0.00148464, -0.00282428, -0.00200561, -0.00068852,
    0.00343044, 0.00902093, 0.01367867, 0.01147965, 0.0027259,
    -0.01766614, -0.04244429, -0.0577468, -0.05245161, -0.01072754,
    0.0732564, 0.17643278, 0.25582214, 0.28200453, 0.25582214,
    0.17643278, 0.0732564, -0.01072754, -0.05245161, -0.0577468,
    -0.04244429, -0.01766614, 0.0027259, 0.01147965, 0.01367867,
    0.00902093, 0.00343044, -0.00068852, -0.00200561, -0.00282428,
    -0.00148464, 0.00119025,
];

struct HdlcDecoder {
    shift_reg: u32,  // sliding window for flag detection (need 8+ bits)
    byte_acc: u8,    // byte accumulator (LSB-first)
    bit_count: u8,   // bits accumulated in byte_acc
    frame_buf: Vec<u8>,
    ones_count: u8,
    in_frame: bool,
    prev_bit: bool,
}

impl HdlcDecoder {
    fn new() -> Self {
        Self { shift_reg: 0, byte_acc: 0, bit_count: 0, frame_buf: Vec::with_capacity(256),
               ones_count: 0, in_frame: false, prev_bit: false }
    }

    fn feed(&mut self, sample: f32, out: &mut Vec<String>, stats: &mut (u64, u64)) {
        let raw_bit = sample > 0.0;
        // NRZI: no transition = 1, transition = 0
        let bit: u8 = if raw_bit == self.prev_bit { 1 } else { 0 };
        self.prev_bit = raw_bit;

        // Shift register for flag detection (pre-unstuffing)
        self.shift_reg = (self.shift_reg << 1) | bit as u32;

        if (self.shift_reg & 0xFF) as u8 == 0x7E {
            if self.in_frame && self.frame_buf.len() >= 5 {
                // CRC: run reflected poly over entire frame, expect 0x0F47
                let mut crc: u16 = 0xFFFF;
                for &b in &self.frame_buf {
                    crc ^= b as u16;
                    for _ in 0..8 {
                        crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
                    }
                }
                if crc == 0x0F47 {
                    stats.0 += 1;
                    let plen = self.frame_buf.len() - 2;
                    let nmea = payload_to_nmea_chars(&self.frame_buf[..plen]);
                    if !nmea.is_empty() {
                        let body = format!("AIVDM,1,1,,A,{},0", nmea);
                        let cs: u8 = body.bytes().fold(0, |a, b| a ^ b);
                        out.push(format!("!{}*{:02X}", body, cs));
                    }
                } else {
                    stats.1 += 1;
                }
            }
            self.in_frame = true;
            self.frame_buf.clear();
            self.ones_count = 0;
            self.bit_count = 0;
            return;
        }
        if !self.in_frame { return; }
        if bit == 1 {
            self.ones_count += 1;
            if self.ones_count > 6 { self.in_frame = false; return; }
        } else {
            if self.ones_count == 5 { self.ones_count = 0; return; }
            self.ones_count = 0;
        }
        self.byte_acc = (self.byte_acc << 1) | bit;
        self.bit_count += 1;
        if self.bit_count >= 8 {
            self.frame_buf.push(self.byte_acc.reverse_bits());
            self.bit_count = 0;
            self.byte_acc = 0;
            if self.frame_buf.len() > 512 { self.in_frame = false; }
        }
    }
}

struct AisChannel {
    rot_re: f64, rot_im: f64,
    rot_inc_re: f64, rot_inc_im: f64,
    // Decimate ÷6 accumulator
    acc_re: f64, acc_im: f64, acc_n: u8,
    // FM discriminator
    fm_prev_re: f64, fm_prev_im: f64, dc_est: f32,
    // FIR filter
    fir_buf: [f32; 37], fir_idx: usize,
    // 5 parallel decoders (brute-force timing)
    decoders: Vec<HdlcDecoder>,
    phase: usize,
}

impl AisChannel {
    fn new(freq_offset: f64, sample_rate: f64) -> Self {
        let angle = 2.0 * std::f64::consts::PI * freq_offset / sample_rate;
        Self {
            rot_re: 1.0, rot_im: 0.0,
            rot_inc_re: angle.cos(), rot_inc_im: angle.sin(),
            acc_re: 0.0, acc_im: 0.0, acc_n: 0,
            fm_prev_re: 1.0, fm_prev_im: 0.0, dc_est: 0.0,
            fir_buf: [0.0; 37], fir_idx: 0,
            decoders: (0..5).map(|_| HdlcDecoder::new()).collect(),
            phase: 0,
        }
    }

    fn process(&mut self, i: f64, q: f64, out: &mut Vec<String>, stats: &mut (u64, u64)) {
        // 1. Rotate to baseband
        let ri = i * self.rot_re - q * self.rot_im;
        let rq = i * self.rot_im + q * self.rot_re;
        let nr = self.rot_re * self.rot_inc_re - self.rot_im * self.rot_inc_im;
        let ni = self.rot_re * self.rot_inc_im + self.rot_im * self.rot_inc_re;
        self.rot_re = nr; self.rot_im = ni;

        // 2. Accumulate for ÷6 decimation → 48 kSPS
        self.acc_re += ri;
        self.acc_im += rq;
        self.acc_n += 1;
        if self.acc_n < 6 { return; }
        let ore = self.acc_re / 6.0;
        let oim = self.acc_im / 6.0;
        self.acc_re = 0.0; self.acc_im = 0.0; self.acc_n = 0;

        // 3. FM discriminator
        let cross = oim * self.fm_prev_re - ore * self.fm_prev_im;
        let dot = ore * self.fm_prev_re + oim * self.fm_prev_im;
        let fm = (cross.atan2(dot) / std::f64::consts::PI) as f32;
        // DC removal (single-pole IIR high-pass, alpha=0.999)
        let dc_removed = fm - self.dc_est;
        self.dc_est += 0.001 * (fm - self.dc_est);
        self.fm_prev_re = ore; self.fm_prev_im = oim;

        // 4. Matched filter (37-tap FIR)
        self.fir_buf[self.fir_idx] = fm;
        self.fir_idx = (self.fir_idx + 1) % 37;
        let mut filtered: f32 = 0.0;
        for t in 0..37 {
            filtered += FILTER_TAPS[t] * self.fir_buf[(self.fir_idx + t) % 37];
        }

        // 5. Deinterleave to 5 parallel decoders
        self.decoders[self.phase].feed(dc_removed, out, stats);
        self.phase = (self.phase + 1) % SAMPLES_PER_BIT;
    }

    fn normalize(&mut self) {
        let m = (self.rot_re * self.rot_re + self.rot_im * self.rot_im).sqrt();
        if m > 0.0 { self.rot_re /= m; self.rot_im /= m; }
    }
}

struct AisState {
    ch_a: AisChannel,
    ch_b: AisChannel,
    count: u64,
    frames_ok: u64,
    frames_crc_fail: u64,
}

impl AisState {
    fn new(sr: u32) -> Self {
        let s = sr as f64;
        Self {
            ch_a: AisChannel::new(0.0, s),
            ch_b: AisChannel::new(0.0, s),
            count: 0, frames_ok: 0, frames_crc_fail: 0,
        }
    }
}

fn demod_ais_chunk(iq: &[u8], state: &mut AisState) -> Vec<String> {
    let mut out = Vec::new();
    let mut stats = (state.frames_ok, state.frames_crc_fail);
    let samples = iq.len() / 2;
    for s in 0..samples {
        let i = (iq[s*2] as f64 - 127.5) / 128.0;
        let q = (iq[s*2+1] as f64 - 127.5) / 128.0;
        state.ch_a.process(i, q, &mut out, &mut stats);
        state.ch_b.process(i, q, &mut out, &mut stats);
        state.count += 1;
        if state.count % 10000 == 0 {
            state.ch_a.normalize();
            state.ch_b.normalize();
        }
    }
    state.frames_ok = stats.0;
    state.frames_crc_fail = stats.1;
    out
}
fn payload_to_nmea_chars(data: &[u8]) -> String {
    let total_bits = data.len() * 8;
    let chars = total_bits / 6;
    let mut out = String::with_capacity(chars);
    for i in 0..chars {
        let bo = i * 6;
        let bi = bo / 8;
        let br = bo % 8;
        let val = if br <= 2 {
            (data[bi] >> (2 - br)) & 0x3F
        } else {
            let mut v = (data[bi] << (br - 2)) & 0x3F;
            if bi + 1 < data.len() { v |= data[bi + 1] >> (10 - br); }
            v
        };
        out.push((if val < 40 { val + 48 } else { val + 56 }) as char);
    }
    out
}

// ===== Main =====
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let duration = args.iter().position(|a| a == "--duration")
        .and_then(|i| args.get(i+1))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60);
    let ais_serial = args.iter().position(|a| a == "--ais").and_then(|i| args.get(i+1).cloned());
    let adsb_serial = args.iter().position(|a| a == "--adsb").and_then(|i| args.get(i+1).cloned());
    let from_stdin = args.iter().any(|a| a == "--stdin");
    let mode_1090 = args.iter().any(|a| a == "--1090");
    let mode_162 = args.iter().any(|a| a == "--162");

    if from_stdin {
        if mode_1090 {
            run_1090_stdin();
        } else if mode_162 {
            run_162_stdin();
        } else {
            eprintln!("--stdin requires --1090 or --162");
            std::process::exit(1);
        }
        return;
    }

    if ais_serial.is_none() && adsb_serial.is_none() {
        eprintln!("Usage: sdr-test [--adsb SERIAL] [--ais SERIAL] [--duration SECS]");
        eprintln!("       sdr-test --stdin --1090    (read IQ from stdin, demod 1090)");
        eprintln!("       sdr-test --stdin --162     (read IQ from stdin, demod AIS)");
        eprintln!("Requires rtl_sdr in PATH.");
        let _ = Command::new("rtl_test").arg("-t").status();
        std::process::exit(1);
    }

    if let Some(ref serial) = adsb_serial {
        run_1090(serial, duration);
    }
    if let Some(ref serial) = ais_serial {
        run_162(serial, duration);
    }
}

fn run_1090_stdin() {
    eprintln!("[1090] Reading IQ from stdin (2 MSPS expected)");
    let stdin = std::io::stdin();
    let mut reader = std::io::BufReader::with_capacity(524288, stdin.lock());
    let lut = MagLut::new();
    let mut mag = vec![0u16; 262144];
    let mut icaos = HashSet::<String>::new();
    let mut total_frames = 0u64;
    let mut buf = vec![0u8; 524288];
    let start = Instant::now();

    loop {
        let n = reader.read(&mut buf).unwrap_or(0);
        if n == 0 { break; }
        let frames = demod_1090_chunk(&buf[..n], &lut, &mut mag);
        for (df, frame, bits) in &frames {
            total_frames += 1;
            let icao = if *df == 17 || *df == 18 || *df == 11 {
                ((frame[1] as u32) << 16) | ((frame[2] as u32) << 8) | frame[3] as u32
            } else {
                modes_crc(&frame[..bits/8]) & 0xFFFFFF
            };
            icaos.insert(format!("{:06x}", icao));
            let hex: String = frame[..bits/8].iter().map(|b| format!("{:02x}", b)).collect();
            println!("*{};", hex);
        }
    }
    eprintln!("[1090] {} frames, {} unique aircraft in {:.0}s", total_frames, icaos.len(), start.elapsed().as_secs_f64());
}

fn run_162_stdin() {
    eprintln!("[162] Reading IQ from stdin (288 kSPS expected)");
    let stdin = std::io::stdin();
    let mut reader = std::io::BufReader::with_capacity(65536, stdin.lock());
    let mut state = AisState::new(288000);
    let mut buf = vec![0u8; 65536];
    let mut total = 0u64;
    let start = Instant::now();

    loop {
        let n = reader.read(&mut buf).unwrap_or(0);
        if n == 0 { break; }
        let sentences = demod_ais_chunk(&buf[..n], &mut state);
        for s in &sentences {
            total += 1;
            println!("{}", s);
        }
    }
    eprintln!("[162] {} sentences, {} CRC ok, {} CRC fail in {:.0}s",
        total, state.frames_ok, state.frames_crc_fail, start.elapsed().as_secs_f64());
}

fn run_1090(serial: &str, duration: u64) {
    let samples = 2_000_000 * duration;  // 2 MSPS * seconds
    let bytes = samples * 2;  // I+Q
    eprintln!("[1090] Capturing {}s from device serial={} (2 MSPS, gain 49.6)", duration, serial);

    let mut child = Command::new("rtl_sdr")
        .args(["-d", &format!("serial:{}", serial)])  // Select by serial
        .args(["-f", "1090000000"])
        .args(["-s", "2000000"])
        .args(["-g", "49.6"])
        .args(["-n", &bytes.to_string()])
        .arg("-")  // output to stdout
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to run rtl_sdr");

    let stdout = child.stdout.take().unwrap();
    let lut = MagLut::new();
    let mut mag = Vec::with_capacity(262144);
    mag.resize(262144, 0u16);
    let mut icaos = HashSet::<String>::new();
    let mut total_frames = 0u64;
    let mut buf = vec![0u8; 524288]; // 256K samples per read
    let mut reader = std::io::BufReader::with_capacity(524288, stdout);
    let start = Instant::now();

    loop {
        let n = reader.read(&mut buf).unwrap_or(0);
        if n == 0 { break; }
        let frames = demod_1090_chunk(&buf[..n], &lut, &mut mag);
        for (df, frame, bits) in &frames {
            total_frames += 1;
            let icao = if *df == 17 || *df == 18 || *df == 11 {
                ((frame[1] as u32) << 16) | ((frame[2] as u32) << 8) | frame[3] as u32
            } else {
                modes_crc(&frame[..bits/8]) & 0xFFFFFF
            };
            icaos.insert(format!("{:06x}", icao));
            let hex: String = frame[..bits/8].iter().map(|b| format!("{:02x}", b)).collect();
            println!("*{};", hex);
        }
    }
    let _ = child.wait();
    eprintln!("[1090] {} frames, {} unique aircraft in {:.0}s", total_frames, icaos.len(), start.elapsed().as_secs_f64());
}

fn run_162(serial: &str, duration: u64) {
    let samples = 288_000u64 * duration;
    let bytes = samples * 2;
    eprintln!("[162] Capturing {}s from device serial={} (288 kSPS, gain 49.6)", duration, serial);

    let mut child = Command::new("rtl_sdr")
        .args(["-d", &format!("serial:{}", serial)])
        .args(["-f", "162000000"])
        .args(["-s", "288000"])
        .args(["-g", "49.6"])
        .args(["-n", &bytes.to_string()])
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to run rtl_sdr");

    let stdout = child.stdout.take().unwrap();
    let mut state = AisState::new(288000);
    let mut buf = vec![0u8; 65536];
    let mut reader = std::io::BufReader::with_capacity(65536, stdout);
    let mut total = 0u64;
    let start = Instant::now();

    loop {
        let n = reader.read(&mut buf).unwrap_or(0);
        if n == 0 { break; }
        let sentences = demod_ais_chunk(&buf[..n], &mut state);
        for s in &sentences {
            total += 1;
            println!("{}", s);
        }
    }
    let _ = child.wait();
    eprintln!("[162] {} sentences, {} CRC ok, {} CRC fail in {:.0}s",
        total, state.frames_ok, state.frames_crc_fail, start.elapsed().as_secs_f64());
}
