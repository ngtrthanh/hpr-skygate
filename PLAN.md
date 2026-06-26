# fa-rs-v4 Plan: Clean Decoder Port from readsb C

## Why v3 failed
- CPR global decode produces garbage positions (11k aircraft vs real ~47 in Bangkok)
- No position sanity checks (range, speed, duplicate pair validation)
- No CRC check on DF0/4/5/11/16/20/21 (only DF17/18 was validated)
- Category encoding bugs
- Accepted frames without proper validation pipeline

## v4 approach: port readsb's decode pipeline step-by-step with tests

### Architecture: same as v2 mio fan-out + optional decode module
```
fa-rs-v4/
├── src/
│   ├── main.rs, ingest.rs, writer.rs, beast.rs, ...  (copy from v2 unchanged)
│   └── decode/
│       ├── mod.rs          — public API: decode_beast_frame() -> Option<AircraftUpdate>
│       ├── crc.rs          — Mode-S 24-bit CRC (lookup table, fast)
│       ├── mode_s.rs       — DF parsing: extract ICAO, payload fields
│       ├── adsb.rs         — DF17/18 TC decoding: ident, position, velocity, status
│       ├── cpr.rs          — CPR position decode (global + local + validation)
│       ├── aircraft.rs     — per-ICAO state machine with timeout/filtering
│       └── output.rs       — JSON + binCraft serialization
└── tests/
    ├── crc_test.rs         — known CRC vectors from readsb test suite
    ├── cpr_test.rs         — known position pairs → expected lat/lon
    ├── decode_test.rs      — raw beast frames → expected decoded fields
    └── integration_test.rs — replay recorded data, compare output vs readsb
```

### Implementation phases (each must pass tests before next)

#### Phase 1: CRC (1 hour)
- Port `crc.c` lookup table (not bit-by-bit loop — readsb uses precomputed table)
- Test: verify known messages produce correct CRC residuals
- Gate: ALL messages must pass CRC before decode

#### Phase 2: DF parsing (2 hours)
- DF11: extract ICAO only (for aircraft existence)
- DF17/18: extract ICAO, TC, ME payload
- DF4/5/20/21: extract altitude/squawk via CRC residual → ICAO
- Test: raw hex messages → expected ICAO + DF + fields
- Gate: reject frames with bad CRC

#### Phase 3: ADS-B decode — ident + velocity (2 hours)
- TC 1-4: callsign + category
- TC 19: ground speed, track, vertical rate, IAS/TAS/heading
- Test: known TC19 messages → expected speed/heading values
- No position decode yet — just metadata

#### Phase 4: CPR position decode (4 hours) — THE CRITICAL PART
Port from readsb `cpr.c` with these specific validations:
1. **Global decode**: requires even+odd pair within 10s, same zone check (NL must match)
2. **Local decode**: requires a reference position (receiver or previous aircraft position)
3. **Range check**: reject positions > max_range from receiver
4. **Speed check**: reject positions that imply > 700kt between updates
5. **Surface vs airborne**: different NL table, different resolution
- Test: known even/odd CPR pairs → expected lat/lon (use readsb test vectors)
- Test: verify positions outside max_range are rejected
- Test: verify speed-check catches jumps

#### Phase 5: Aircraft state machine (2 hours)
- Per-ICAO HashMap with timeout (60s no message → remove)
- Only create entry from DF17/18 (not DF11)
- Position only accepted after CPR validation passes
- Message counter, last_seen, signal level tracking
- Test: replay sequence of messages, verify state transitions

#### Phase 6: Output (1 hour)
- JSON aircraft.json format (matching readsb field names exactly)
- binCraft 112-byte records (from skylink-core, already verified)
- Test: compare output for known aircraft vs readsb reference

#### Phase 7: Integration + benchmark (2 hours)
- Capture 60s of beast data from readsb (`nc localhost 30005 > test.beast`)
- Replay through fa-rs-v4, compare aircraft count vs readsb
- Must be within 5% of readsb aircraft count
- Benchmark: target <40% CPU with decode on

### Key readsb C files to port from
| File | What | Critical functions |
|------|------|--------------------|
| `crc.c` | CRC table + compute | `modesChecksum()` |
| `mode_s.c` | DF parsing | `decodeModesMessage()` |
| `cpr.c` | Position decode | `decodeCPRrelative()`, `decodeCPRairborne()` |
| `track.c` | State machine | `trackUpdateFromMessage()`, speed/range checks |

### Test data sources
1. Record beast from readsb: `timeout 60 nc localhost 30005 > /tmp/test_60s.beast`
2. Get readsb's aircraft.json at same time for comparison
3. Use known test vectors from dump1090/readsb test suites

### Success criteria
- Bangkok area: ~40-60 aircraft (matching readsb ±10%)
- Global: aircraft count within 5% of readsb
- No garbage positions (all positions within declared max_range or validated by speed check)
- Squawks correct (4 octal digits)
- Categories correct (A0-D7 format)
- CPU: <45% with decode enabled at 200k msg/s
