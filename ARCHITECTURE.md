# hpr-skygate: Antenna-to-Browser Architecture

## Vision

One Rust binary. Two antennas. Aviation + Maritime. SDR to map.
Replaces: readsb + dump1090 + AIS-catcher + hpr-atlas + tar1090.

## Two Products, One Codebase

| | **Aggregator** (server) | **Edge** (feeder station) |
|--|------------------------|--------------------------|
| Runs at | HPRadar data center | Feeder's home (Pi/NUC) |
| Inputs | 2500+ beast TCP + AIS TCP | Local SDR (1090 + 162 MHz) |
| Role | Fuse, deduplicate, serve global | Decode, feed upstream, serve local |
| Frontend | Global map (10k+ targets) | Personal radar (50-500 targets) |
| License | Proprietary / AGPL | MIT (maximum adoption) |

```bash
# Aggregator mode
hpr-skygate --mode aggregator \
  --net-bi-port 30004 --ais-port 5014 --mlat-port 31090 \
  --ws-port 9877 --http 0.0.0.0:8088

# Edge mode
hpr-skygate --mode edge \
  --sdr-device 0 --ais-device 1 \
  --feed-to feed.hpradar.org:30004 --ais-feed-to feed.hpradar.org:5014 \
  --uuid $MY_UUID --http 0.0.0.0:8080
```

## Feeder Identity Model

A superstation feeds ADS-B + AIS + MLAT under one UUID:

```
Feeder UUID: abc-123
├── Beast Reduce Plus → hpradar.org:30004 (ADS-B)
├── NMEA TCP → hpradar.org:5014 (AIS)
└── MLAT → hpradar.org:31090 (future)
```

Skygate merges all feeds from the same UUID into one feeder entity.

## §8 Wire Protocol (shared between aviation + maritime)

```
0x01 Position [19B]: type(1) + id(4) + lon(4) + lat(4) + sog(2) + cog(2) + hdg(2)
0x05 Static   [44B]: type(1) + id(4) + subtype(1) + name(20) + callsign(7) + ...
0x15 AtoN     [34B]: type(1) + id(4) + atontype(1) + lon(4) + lat(4) + name(20)
0x08 Binary   [12+N]: type(1) + id(4) + dac(2) + fid(1) + datalen(2) + subtype(1) + spare(1) + payload
```

Same format for aircraft (ICAO) and vessels (MMSI). Frontend decodes both with one parser.

## WebSocket Hub (from hpr-atlas pattern)

- Binary push (no JSON)
- Delta filter: skip if moved <11m AND speed unchanged AND <30s
- Snapshot on connect: full state dump to new client
- Batch: 50ms accumulate, flush as single WS message
- zstd compression: optional via `?zstd=1`
- Static backfill: emit cached static on first sighting

## Source Layout

```
src/
├── main.rs              # Mode selection (aggregator/edge), wiring
├── config.rs            # CLI flags
├── decode/
│   ├── mod.rs
│   ├── crc.rs           # Mode-S CRC-24
│   ├── cpr.rs           # CPR position decode
│   ├── mode_s.rs        # DF parsing + ADS-B
│   ├── aircraft.rs      # Aircraft state + pos_reliable
│   ├── output.rs        # JSON + binCraft output
│   └── receiver_map.rs  # Self-learning receiver coverage
├── ais/
│   ├── mod.rs
│   ├── decode.rs        # NMEA → §8 binary frames (types 1-27)
│   ├── vessel.rs        # Vessel state map + static cache
│   ├── dedup.rs         # Sentence-level TTL dedup
│   └── demod.rs         # 162 MHz IQ → NMEA (GMSK/NRZI/HDLC)
├── sdr/
│   ├── mod.rs
│   ├── mag.rs           # 1090 MHz magnitude LUT
│   ├── preamble.rs      # Preamble detection + SIMD
│   ├── slicer.rs        # Manchester bit slicer
│   └── rtlsdr.rs        # librtlsdr FFI
├── ingest.rs            # Beast TCP + AIS TCP listener (aggregator)
├── upstream.rs          # Feed-to-server client (edge)
├── ws.rs                # §8 binary WebSocket hub
├── wire.rs              # §8 frame encode/decode
├── enrichment.rs        # Self-learning routes + vessel names
├── feeder.rs            # Unified feeder management
├── alerts.rs            # Monitoring (dropout, stall, anomaly)
├── api.rs               # HTTP endpoints
├── beast.rs             # Beast framer
├── writer.rs            # Fan-out shared buffer
├── client.rs            # Per-feeder connection state
├── receiver.rs          # UUID/receiverId
├── reduce.rs            # Beast-reduce rate limiter
└── sbs.rs               # SBS/BaseStation output
```

## Execution Plan

### Phase A: Absorb AIS (9 hrs)

| # | Task | Hours | Test |
|---|------|-------|------|
| A1 | `src/ais/decode.rs` — NMEA sentence → §8 frames | 2 | Unit test: known NMEA → correct binary |
| A2 | `src/ais/mod.rs` — NMEA TCP source reader (connect, reconnect, stats) | 1.5 | Connect to aishub, verify frames flow |
| A3 | `src/ais/vessel.rs` — vessel state map (merge static, TTL 30min) | 1 | Unit test: merge type 5 + type 24 |
| A4 | `src/ais/dedup.rs` — sentence TTL cache (30s default) | 0.5 | Unit test: same sentence rejected within window |
| A5 | Unified feeder tracking (one UUID → adsb_msgs + ais_msgs) | 1 | API shows combined stats per feeder |
| A6 | `/api/vessels` JSON endpoint | 0.5 | curl returns vessel list with positions |
| A7 | Wire to existing AIS source, end-to-end test | 0.5 | vessels appear in /api/vessels |
| | **Milestone:** `hpr-skygate --ais-sources "hub=data.aishub.net:4541"` serves vessels | | |

### Phase B: §8 WebSocket Hub (2.5 hrs)

| # | Task | Hours | Test |
|---|------|-------|------|
| B1 | Rewrite ws.rs — §8 frames (position + static), not binCraft | 1 | WS client receives 19B position frames |
| B2 | Delta filter: isDominated (moved <11m, speed same, <30s) | 0.5 | Stationary vessel only sent every 30s |
| B3 | Snapshot on connect: full aircraft + vessel state dump | 0.5 | New client immediately sees all targets |
| B4 | Batch (50ms) + zstd (`?zstd=1`) | 0.5 | Measure: bandwidth with/without zstd |
| | **Milestone:** JS `DataView` decodes live aircraft+vessels from one WS | | |

### Phase C: SDR Demod (4 hrs)

| # | Task | Hours | Test |
|---|------|-------|------|
| C1 | Move hpr-demod modules into `src/sdr/` | 0.5 | cargo build still works |
| C2 | `--sdr-device 0` opens 1090 MHz, feeds beast into ingest | 0.5 | (needs antenna) |
| C3 | `src/ais/demod.rs` — FM discriminator for 162 MHz GMSK | 1 | Unit test: synthetic IQ → correct NMEA |
| C4 | Clock recovery + NRZI + HDLC deframe + CRC-16 | 1.5 | Decode known AIS IQ recording |
| C5 | `--ais-device 1` opens 162 MHz, feeds NMEA into AIS pipeline | 0.5 | (needs antenna + AIS traffic) |
| | **Milestone:** one binary decodes both 1090+162 from local SDR | | |

### Phase D: Edge → Aggregator Feed (2 hrs)

| # | Task | Hours | Test |
|---|------|-------|------|
| D1 | `src/upstream.rs` — beast_reduce_plus_out client to remote server | 1 | Edge connects to aggregator, frames flow |
| D2 | AIS upstream: NMEA TCP client with UUID hello | 0.5 | AIS sentences appear on aggregator |
| D3 | `--mode edge` flag wires SDR→decode→upstream + local WS/HTTP | 0.5 | Edge serves local map AND feeds upstream |
| | **Milestone:** edge binary feeds HPRadar + shows local radar | | |

### Phase E: Frontend Shared Core (3 hrs)

| # | Task | Hours | Test |
|---|------|-------|------|
| E1 | `hpr-wire.js` — §8 binary frame decoder (DataView) | 0.5 | Decode synthetic frames correctly |
| E2 | `hpr-map.js` — MapLibre wrapper (add/update/remove targets) | 1.5 | Render 1000 targets smoothly |
| E3 | `hpr-targets.js` — state management (aircraft Map + vessel Map) | 1 | Handle delta + snapshot correctly |
| | **Milestone:** JS library decodes binary WS and renders on map | | |

### Phase F: Aggregator Frontend (4 hrs)

| # | Task | Hours | Test |
|---|------|-------|------|
| F1 | Global map: 10k aircraft + vessels, zoom levels, clustering | 2 | 60 FPS with 10k targets |
| F2 | Info panel: click → details (route, type, history) | 1 | Correct data shown |
| F3 | Stats bar: counts, feeder count, message rate | 0.5 | Updates in real-time |
| F4 | Feeder coverage heatmap overlay | 0.5 | Visual shows global coverage |
| | **Milestone:** hpradar.com global map with planes + ships | | |

### Phase G: Edge Frontend (4 hrs)

| # | Task | Hours | Test |
|---|------|-------|------|
| G1 | Local radar view: centered on station, range rings (50/100/200nm) | 1.5 | Map centers on feeder lat/lon |
| G2 | Personal stats: today's count, range record, unique catches | 1 | Correct tallying |
| G3 | Target list: sortable by distance, altitude, type | 1 | Updates live |
| G4 | System status: SDR health, connection status, CPU/RAM | 0.5 | Accurate readings |
| | **Milestone:** feeder owner's personal radar at http://my-pi:8080 | | |

### Phase H: MLAT Wire (2 hrs)

| # | Task | Hours | Test |
|---|------|-------|------|
| H1 | MLAT port (:31090) accepts mlat-client connections, proxies to mlat-server | 1 | Clients connect successfully |
| H2 | MLAT results (beast frames) ingested back, tagged source=mlat, attributed to feeders | 1 | MLAT positions show in aircraft state |
| | **Milestone:** MLAT feeders get attribution credit | | |

## Total: 30.5 hours

## Licensing

| Component | License | Why |
|-----------|---------|-----|
| Core decode + SDR + edge mode | MIT | Max feeder adoption |
| §8 wire protocol | MIT | Open standard, ecosystem grows |
| Aggregator features (fusion, feeder mgmt, alerts) | Proprietary | Business moat |
| Aggregator frontend | Proprietary | Brand + experience |
| Edge frontend | MIT | Feeders love it → more feeders for you |

## Moats (not in the code)

1. 2500+ feeders (network effect)
2. Self-learned receiver map (1940 receivers × coverage)
3. 85k+ cached routes (from traffic-api)
4. Multi-source agreements (AISHub, etc.)
5. Brand + domain (hpradar.com)
6. Community trust
