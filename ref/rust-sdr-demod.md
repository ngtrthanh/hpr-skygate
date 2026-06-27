---

## 1. System Block Diagram (Text-based Architecture)

```
+---------------------------------------------------------------------------------+
|                                 HIGH-SPEED HARDWARE I/O                         |
|  [ RTL-SDR / Airspy Device ] ---> (USB / DMA via librtlsdr)                       |
+---------------------------------------------------------------------------------+
                                       |
                                       v [Continuous Raw Stream: Interleaved u8 I/Q Packets]
+---------------------------------------------------------------------------------+
|                      STAGE 1: ASYNC INGESTION & BUFFERING (Rust)                |
|  - Multi-threaded Lock-free Ring Buffer (via crossbeam-channel)                |
|  - Double Buffering (Chunk size: 256KB to match USB endpoints)                  |
+---------------------------------------------------------------------------------+
                                       |
                                       v [Chunk Hand-off]
+---------------------------------------------------------------------------------+
|                      STAGE 2: HARDWARE-ACCELERATED DSP CORE                      |
|  - Magnitude Transformer: I/Q to u16 Amplitude via 64KB Look-Up Table (LUT)    |
|  - Zero-allocation Sliding Window (`.windows(16)` with SIMD/AVX2 Auto-vector)  |
+---------------------------------------------------------------------------------+
                                       |
                                       v [u16 Magnitude Array]
+---------------------------------------------------------------------------------+
|                      STAGE 3: SYNC & DEMODULATION ENGINE                         |
|  - Preamble Detector: Validates 4-pulse pattern (samples 0, 2, 7, 9)            |
|  - Dynamic Threshold Noise Estimator                                            |
|  - Manchester Bit Slicer: 2-sample delta analysis -> 112-bit Raw Payload         |
+---------------------------------------------------------------------------------+
                                       |
                                       v [Raw 112-bit Frames + Confidence Array]
+---------------------------------------------------------------------------------+
|                      STAGE 4: BIT INTEGRITY & ERROR RECOVERY                     |
|  - Mode S CRC Parity Checker                                                    |
|  - Targeted 1-bit / 2-bit Aggressive Flipping (on low-confidence samples only)  |
+---------------------------------------------------------------------------------+
                                       |
                                       v [Valid Mode S Frame Bytes]
+---------------------------------------------------------------------------------+
|                      STAGE 5: DOWNSTREAM ADAPTER & PIPELINE                      |
|  - Packs into standard Beast Format (or directly encodes to 11-Byte hpradar frame)|
|  - Zero-copy IPC / TCP Fanout streaming to Go backend/Canvas front-end         |
+---------------------------------------------------------------------------------+

```

---

## 2. Product Requirements Document (PRD)

# Product Requirements Document (PRD)

## Project: `rust-sdr-demod` — High-Performance Mode S/ADS-B IQ Demodulator

### 1. Objective & Scope

The goal of this project is to build a zero-compromise, ultra-high-throughput Mode S (ADS-B) IQ demodulator written entirely in Rust. It directly interfaces with software-defined radio (SDR) hardware to extract raw digital aircraft frames from RF signals, replacing legacy C-based alternatives (`readsb`/`dump1090-fa`) within the unified high-concurrency tracking ecosystem.

### 2. Performance & Target Constraints

* **Target Throughput:** Must natively handle a minimum of **2.0 MSPS to 2.4 MSPS** raw 8-bit unsigned I/Q data stream on a single CPU core without dropping packets.
* **Memory Footprint:** Strict zero-heap-allocation during the hot processing loop. Continuous recycling of memory chunks.
* **Latency:** Sub-millisecond end-to-end processing delay from USB frame ingestion to bit output.
* **No Safety Overheads:** Explicitly bypass index bounds-checking in critical paths using optimized iterators or compiler hints (`target-cpu=native`).

---

### 3. Functional Requirements & Implementation Spec

#### Module 3.1: Async I/O & Double Buffering

* **Input Channel:** Interface with `librtlsdr` via asynchronous callbacks. Collect interleaved `u8` bytes representing alternating $I$ and $Q$ components ($I_0, Q_0, I_1, Q_1 \dots$).
* **Threading Model:** Decouple the hardware I/O thread from the DSP worker threads using a lock-free ring buffer (e.g., `crossbeam-channel`). Block sizes must align to **256 KB** chunks.

#### Module 3.2: High-Speed Magnitude Calculation

* **Mathematical Concept:** Transform IQ coordinates to magnitude via $M = \sqrt{I^2 + Q^2}$.
* **Rust Implementation:**
* Initialize a static 1D or 2D array serving as a **64 KB Look-Up Table (LUT)** containing precomputed `u16` magnitude values scaled from the `[-128, 127]` signed offsets.
* The worker loop must pull $I$ and $Q$, combine them into a 16-bit lookup index, and extract the magnitude with **$O(1)$ complexity**.
* *AI Hint:* Structure the loop to allow the rust compiler to leverage **auto-vectorization (SIMD AVX2/NEON)**.



#### Module 3.3: Preamble Detection & Synchronization

* **Window Size:** Operate a 16-sample sliding window (corresponding to the 8 $\mu s$ preamble at 2 MSPS).
* **Pulse Validation:** Assert validation rules over the window:
* Magnitudes at samples `0, 2, 7, 9` (pulses) must be strictly greater than samples `1, 3, 8, 10`.
* The combined magnitude of the 4 peak pulses must exceed the dynamic noise floor (calculated from the remaining 12 valley samples) by a runtime-configurable factor.



#### Module 3.4: Manchester Demodulation & Slicing

* **Bit Decoding:** Once synchronized, parse the trailing 112-bit message body (224 samples at 2 MSPS).
* **Rule Engine:** Evaluate consecutive 2-sample segments $(S_1, S_2)$ representing 1 $\mu s$:
* If $S_1 > S_2 \implies \text{Bit } 1$
* If $S_1 < S_2 \implies \text{Bit } 0$
* If $|S_1 - S_2| \le \text{Threshold} \implies$ Mark bit index as **Low Confidence** for the error correction matrix.


* **Bit Packing:** Shift decoded bits directly into a compact `[u8; 14]` array.

#### Module 3.5: Parity Verification & Error Correction

* **CRC Execution:** Compute Mode S cyclic redundancy check parity over the packet.
* **Error Recovery:**
* *Level 1:* If CRC fails, attempt single-bit flipping across all 112 bits sequentially.
* *Level 2 (Aggressive):* If Level 1 fails, leverage the **Low Confidence** array to perform targeted 2-bit combinations flipping to preserve CPU cycles from blind iterations.



---

### 4. Non-Functional & Code Style Guardrails (For AI Agent)

* **Zero-Panic Guarantee:** No code in the hot loop should trigger a `panic!`. Avoid unsafe unwraps; utilize compile-time fixed-size array matching instead.
* **No Allocation:** No vectors (`Vec`), Strings, or dynamic heap data types are permitted inside the processing loops.
* **Benchmarking Requirement:** Provide a `criterion`-based micro-benchmark suite parsing raw `.iq` dump files to track throughput metrics.
* **Output Interfacing:** Format verified frames into standard binary output payloads (e.g., Beast Mode ACAV frame) or map them directly into a raw byte format for downstream Go microservices.