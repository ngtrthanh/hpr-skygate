# hpr-skygate

High-performance ADS-B/Mode-S beast ingest, decode, and fan-out engine. Single-thread mio epoll architecture. Replaces readsb at 3× less CPU and 100× less RAM for aggregated feeder networks.

**Part of [HPRadar](https://hpradar.com)** — Aviation + Maritime intelligence for the open web.

## Performance

Benchmarked at 2,500 feeders, 200k messages/second:

| | hpr-skygate | readsb (C) |
|--|-------------|------------|
| CPU | **33%** | 97% |
| RAM | **117 MB** | 4,000 MB |
| Binary | 1.8 MB | container |
| Aircraft tracked | 11,600+ | 12,300 |
| Position coverage | 92% | 100% |
| Startup | instant | ~30s |

## Features

### Ingest (L1)
- Beast TCP ingest from thousands of feeders
- UUID → receiverId tracking and E3 prefix injection
- Per-feeder rate limiting with garbage routing
- Block/unblock feeders via HTTP API
- dropHalf backpressure (readsb-style)

### Decode (L2) — optional, `--decode` flag
- Full DF0/4/5/11/16/17/18/20/21 parsing
- CRC-24 table-based validation + 1-bit error correction
- ADS-B TC1-31 decode (ident, position, velocity, status)
- CPR global + relative decode with speed check
- BDS 5,0 Comm-B decode (GS/TAS/track from DF20/21)
- Aircraft DB enrichment (566k hex → registration/type)
- Self-learning route enrichment via traffic-api

### Output (L4) — all switchable
- Beast fan-out (`--net-bo-port`)
- Beast-reduce (`--net-beast-reduce-out-port`)
- SBS/BaseStation (`--net-sbs-port`)
- §8 binary frames for hpr-atlas (`--net-atlas-port`)
- JSON aircraft API (`/api/aircraft`)
- binCraft 112B binary (`/api/aircraft.binCraft`, `/re-api/?binCraft`)
- BBox server-side filter (`&box=S,N,W,E`)
- WebSocket binary push (`--ws-port`)
- Flight trace/history (`/api/trace/{hex}`)
- tar1090 static file serving (`--web-dir`)

### Zero-cost switching
```bash
# Pure fan-out mode (18% CPU, 24 MB — no decode overhead)
hpr-skygate --net-bi-port 30004 --net-bo-port 30005 --net-receiver-id

# Full decode mode (33% CPU, 117 MB — aircraft tracking + all outputs)
hpr-skygate --net-bi-port 30004 --net-bo-port 30005 --net-receiver-id \
  --decode --net-sbs-port 30003 --net-beast-reduce-out-port 30006 \
  --net-atlas-port 30007 --ws-port 9877 --http 0.0.0.0:8088 \
  --web-dir /usr/share/tar1090/html
```

## Quick Start

```bash
# Build
cargo build --release --target x86_64-unknown-linux-musl

# Run (fan-out only)
./target/release/hpr-skygate --net-bi-port 30004 --net-bo-port 30005 --net-receiver-id

# Run (full decode + tar1090 web UI)
./target/release/hpr-skygate \
  --net-bi-port 30004 \
  --net-bo-port 30005 \
  --net-receiver-id \
  --decode \
  --http 0.0.0.0:8088 \
  --web-dir /path/to/tar1090/html
```

## Docker

```dockerfile
FROM alpine:3.20
COPY target/x86_64-unknown-linux-musl/release/hpr-skygate /usr/local/bin/
ENTRYPOINT ["hpr-skygate"]
```

```yaml
services:
  skygate:
    build: .
    network_mode: host
    command:
      - --net-bi-port=30004
      - --net-bo-port=30005
      - --net-receiver-id
      - --decode
      - --http=0.0.0.0:8088
```

## Architecture

```
feeders (2500+) ──► hpr-skygate (single thread, mio epoll)
                        │
                        ├── Beast fan-out (:30005)
                        ├── Beast-reduce (:30006)
                        ├── SBS/BaseStation (:30003)
                        ├── §8 binary frames (:30007) ──► hpr-atlas (Go, L3/L4)
                        ├── HTTP API (:8088)
                        │     ├── /health
                        │     ├── /api/aircraft (JSON)
                        │     ├── /api/aircraft.binCraft
                        │     ├── /api/feeders
                        │     ├── /api/trace/{hex}
                        │     ├── /re-api/?binCraft&box=S,N,W,E (tar1090)
                        │     └── / (tar1090 static files)
                        └── WebSocket (:9877) ──► hpr-marine frontend
```

### Design principles
- **Single-thread mio** — no context switching, no lock contention
- **Shared writer buffer** — batch frames, flush every 1280 bytes or 50ms
- **Per-client sendq + dropHalf** — graceful degradation, not disconnect
- **CRC before parse** — reject garbage frames before decode cost
- **Speed check without receiver location** — works for global aggregators
- **Self-learning enrichment** — no pre-loaded route CSV, learns from traffic

## API

### Health
```
GET /health
{"status":"ok","feeders":2534,"uptime_seconds":3600.0}
```

### Aircraft
```
GET /api/aircraft
GET /api/aircraft?box=12.7,14.7,99.5,101.5
{"now":1782400000.0,"messages":50000000,"aircraft":[...]}
```

### binCraft (tar1090 compatible)
```
GET /api/aircraft.binCraft
GET /re-api/?binCraft&box=12.7,14.7,99.5,101.5
```

### Trace
```
GET /api/trace/885102
[[1782400000.1,13.92,100.59,15650,248.3],...]
```

### Feeders
```
GET /api/feeders
POST /api/feeders/block {"id":"192.168.1.100"}
POST /api/feeders/unblock {"id":"192.168.1.100"}
```

## CLI Reference

| Flag | Default | Description |
|------|---------|-------------|
| `--net-bi-port` | 30004 | Beast input (feeders connect here) |
| `--net-bo-port` | 30005 | Beast output (subscribers) |
| `--net-receiver-id` | true | Prepend 0xe3 receiverId |
| `--net-ingest` | true | Enable rate limiting |
| `--net-garbage-port` | — | Garbage output port |
| `--net-beast-reduce-out-port` | — | Beast-reduce output |
| `--beast-reduce-interval` | 0.25 | Reduce interval (seconds) |
| `--net-sbs-port` | — | SBS/BaseStation output |
| `--net-atlas-port` | — | §8 binary frames for hpr-atlas |
| `--net-heartbeat` | 60 | Heartbeat interval (seconds) |
| `--decode` | false | Enable Mode-S decode |
| `--http` | 0.0.0.0:9876 | HTTP API listen address |
| `--ws-port` | — | WebSocket binary push port |
| `--web-dir` | — | Serve static files (tar1090) |

## Integration with HPRadar

hpr-skygate is L1+L2 in the [HPRadar canonical architecture](https://github.com/ngtrthanh/hpr-atlas):

- **Feeds hpr-atlas** via `--net-atlas-port` (§8 binary frames)
- **Enriches from hpr-traffic-api** via self-learning route cache
- **Serves tar1090** directly via `--web-dir`
- **Feeds hpr-marine** frontend via `--ws-port`

## License

MIT

---

Built by [HPRadar](https://hpradar.com)
