# AIS 162 MHz Demod Improvement Plan

## What ais-catcher does (from source analysis)

The default model (`ModelDefault`) pipeline at 288 kSPS:

```
IQ @ 288 kSPS
  → DownsampleKFilter(Blackman-Harris, ÷3) → 96 kSPS
  → Rotate(±25kHz) → splits Ch A (161.975) / Ch B (162.025)
  → Downsample2CIC5 → 48 kSPS (= 5 samples/bit at 9600 baud)
  → FilterCIC5 (anti-alias cleanup)
  → SquareFreqOffsetCorrection (AFC: FFT-based freq offset est)
  → FilterComplex (17-tap "Coherent" matched filter)
  → ScatterPLL (distributes to 5 phase offsets)
  → PhaseSearchEMA (coherent bit decision with 16-phase rotation)
  → NRZI decode
  → AIS::Decoder (HDLC deframe + CRC)
```

### Key insights vs our implementation:

1. **Channel separation**: Tunes to 162.0 MHz, then rotates ±25 kHz to split channels A/B.
   We try to demod the whole 288 kHz band as one — wrong!

2. **Downsample to 48 kSPS** (5 samples/bit): This gives exactly 5 sample positions per bit.
   We run at full sample rate with a free-running clock — too many samples, no structure.

3. **Brute-force timing recovery** via `Deinterleave` + 5 parallel decoders:
   Instead of a PLL, it runs 5 decoders at different sample offsets (0, 1, 2, 3, 4 out of 5).
   One of them will always be "correct". NO timing PLL needed!

4. **Coherent demod** via `PhaseSearchEMA`:
   - Rotates IQ by (1j)^i to align GMSK constellation
   - Tests 16 phase hypotheses simultaneously
   - Picks the one with highest EMA energy
   - Does differential NRZI in the decision

5. **Automatic Frequency Correction** (`SquareFreqOffsetCorrection`):
   - FFT-based carrier offset estimation
   - Compensates for dongle LO drift

6. **Matched filter** (`Filters::Coherent`): 17-tap Gaussian pulse shape filter.

## Implementation plan for skygate

### Step 1: Channel separation (Rotate ±25 kHz)
At 288 kSPS, rotation = e^(j*2π*25000/288000) per sample.
Split into ch_a and ch_b.

### Step 2: Downsample to 48 kSPS (÷6 from 288k, or ÷3 then ÷2)
Use CIC5 decimator (simple accumulate + decimate).

### Step 3: Brute-force timing (5 parallel decoders)
At 48 kSPS / 9600 baud = 5 samples per bit.
Run 5 decoders, each starting at offset 0-4.
Each decoder takes every 5th sample.

### Step 4: FM discriminator + matched filter
For each sample: `output = atan2(cross, dot)` then convolve with Receiver filter (37 taps).

### Step 5: NRZI → HDLC → CRC
Standard: transition=0, no-transition=1. Then HDLC deframe with bit-unstuffing.
CRC-16 reflected poly 0x8408.

### Simplified approach (what "Standard" model does)
The `ModelStandard` is simpler and still works:
```
48 kSPS complex → FM discriminator → 37-tap FIR filter → Deinterleave(5) → 5× Decoder
```
This is the NON-coherent model. Easier to implement. Still beats our current approach.

## Critical differences from our code:
| | Our code | ais-catcher |
|--|----------|-------------|
| Channel sep | None (full band) | ±25kHz rotation |
| Sample rate at demod | 288 kSPS | 48 kSPS |
| Timing | Free-running clock | 5 parallel decoders (brute force) |
| Filter | Moving average | 37-tap matched filter |
| Bit decision | Threshold on single sample | Filtered + multi-phase search |
| AFC | None | FFT-based freq correction |

## Minimum viable fix
Port the `ModelStandard` approach:
1. Rotate to split channels
2. Decimate to 48 kSPS
3. FM discriminator (atan2)
4. 37-tap FIR (Receiver filter)
5. Feed every 5th sample to 5 HDLC decoders
6. Each decoder does simple threshold → NRZI → HDLC

This avoids the complex coherent PhaseSearch but should decode most packets.
