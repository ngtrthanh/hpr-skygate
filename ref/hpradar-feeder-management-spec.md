# hpradar — Feeder Management Implementation Spec

**Audience:** coding agent.
**Scope:** feeder identity (UUID), client side, decoder + ingest, per-feeder metrics. The engagement/dashboard layer is referenced but specified separately.

**Source/label note**
- Protocol and tool facts cite upstream docs (readsb, ultrafeeder, AIS-catcher) and are linked inline.
- Architecture decisions are **[Inference]** — reasoned from how existing community aggregators (adsb.fi, adsb.lol, airplanes.live, AIS-catcher community feed) operate, not validated for hpradar's deployment.
- Items marked **[Unverified – confirm]** are wire-format/flag specifics the agent must verify against current upstream source before relying on them. Do not treat them as settled.
- LLM/automated-behavior claims, where any appear, are not assured outcomes.

---

## 0. Guiding principles [Inference]

1. **Adopt ecosystem standards, do not invent a protocol.** ADS-B/MLAT new-aggregator standard is **Beast Reduce Plus (BRP)** = Beast format + UUID + duplicate reduction (`beast_reduce_plus_out`). AIS standard is **AIS-catcher** NMEA over HTTP/UDP/TCP. Speaking these makes onboarding a one-line config change for any existing feeder.
2. **One scoring engine, two views.** Ops monitoring and the feeder-facing "addictive" dashboard read the *same* per-feeder rollups. Build the rollups once.
3. **Feed-first, claim-later.** A feeder can start sending data anonymously with just a UUID; they bind it to an account afterward. Lowest onboarding friction.
4. **UUID is feed identity, not authorization.** A bearer UUID identifies a stream. Account actions and perks (API quota, premium) require a separate issued token gated on a trust score. This addresses the "perks get farmed" risk.
5. **Location privacy by default.** Public station location is fuzzed; exact antenna position is visible only to the ops layer and the owner.

---

## 1. Identity & UUID model

### 1.1 Three tiers

| Tier | Entity | Cardinality | Purpose |
|---|---|---|---|
| 1 | `account` | 1 human/owner | login, perks, ownership |
| 2 | `station` | N per account | a physical site/antenna; public on map |
| 3 | `feed_source` | N per station | one stream of one kind (adsb / mlat / ais / uat); **keyed by the feeder-supplied UUID** |

A single box commonly produces several `feed_source` rows: e.g. one station running ADS-B + MLAT + AIS = three feed_sources.

### 1.2 UUID rules

- **Format:** standard UUID (v4), the ecosystem norm generated on the feeder by `cat /proc/sys/kernel/random/uuid`. hpradar must accept any RFC-4122 UUID a client supplies. (Ref: ultrafeeder / multifeeder docs.)
- **The feeder-supplied UUID is the natural key of `feed_source`.** Do not remap it; you need it to match what the feeder sends on the wire.
- **Per-aggregator UUID is recommended to feeders for privacy** — reusing one UUID across aggregators lets sites correlate a feeder's presence. hpradar issues/accepts a hpradar-specific UUID. (Ref: ultrafeeder gitbook privacy note.)
- **Internal surrogate keys:** if you want time-ordered PKs for joins/partitioning, store a server-side `id` (UUIDv7 or bigint) alongside the feeder UUID; never expose it on the wire.
- **Auth separation:** UUID alone grants *data ingest at provisional trust*. An `ingest_token` (per feed_source, issued at claim) authenticates account/perk actions. Perks gate on `station.claimed = true AND trust_score >= threshold`.

### 1.3 Feed-first / claim-later flow [Inference]

```
Unknown UUID connects to ingest
  -> auto-create station(status='provisional', account_id=NULL)
     + feed_source(uuid=<supplied>, kind=<from port/endpoint>, trust='new')
  -> data accepted into a quarantined-trust lane (counts toward coverage,
     excluded from leaderboards until plausibility passes)

Feeder visits dashboard, requests a claim code
  -> claim_code bound to station, short TTL
  -> feeder enters code (or confirms via the UUID they control)
  -> station.account_id set, status='claimed', ingest_token issued
```

This mirrors how adsb.fi / adsb.lol / AIS-catcher community feed let people feed before registering.

### 1.4 Schema (PostgreSQL + TimescaleDB)

```sql
CREATE TABLE account (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  email         text UNIQUE NOT NULL,
  display_name  text,
  created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE station (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  account_id    uuid REFERENCES account(id),          -- NULL while provisional
  name          text,                                  -- [A-Za-z0-9_-], no spaces
  lat           double precision,
  lon           double precision,
  alt_m         double precision,
  public_fuzz_m integer NOT NULL DEFAULT 1000,         -- public location jitter radius
  status        text NOT NULL DEFAULT 'provisional',   -- provisional|claimed|disabled
  created_at    timestamptz NOT NULL DEFAULT now(),
  claimed_at    timestamptz
);

CREATE TABLE feed_source (
  uuid          uuid PRIMARY KEY,                       -- feeder-supplied, wire identity
  station_id    uuid NOT NULL REFERENCES station(id),
  kind          text NOT NULL,                          -- adsb|mlat|ais|uat
  ingest_token  text,                                   -- issued at claim; perk auth
  trust_score   real NOT NULL DEFAULT 0,                -- 0..1
  enabled       boolean NOT NULL DEFAULT true,
  first_seen    timestamptz NOT NULL DEFAULT now(),
  last_seen     timestamptz
);

CREATE TABLE claim_code (
  code          text PRIMARY KEY,
  station_id    uuid NOT NULL REFERENCES station(id),
  expires_at    timestamptz NOT NULL
);
```

Metrics tables are in §4.

---

## 2. Client side

**Recommendation [Inference]: do not build a bespoke feeder app.** Lean on the maintained ecosystem clients and ship a thin hpradar config layer (and optionally a prebuilt image later). This is less code for you and zero learning curve for feeders who already run these.

### 2.1 ADS-B + MLAT client — Ultrafeeder

The feeder runs `ghcr.io/sdr-enthusiasts/docker-adsb-ultrafeeder` (readsb + mlat-client + tar1090 + graphs1090). They add hpradar as one more output. (Ref: docker-adsb-ultrafeeder README.)

UUID persistence on the box: generate once, store, reuse. The ecosystem keeps it in the compose `.env` (`UUID=...`) or a `--uuid-file`.

`.env` (no `version:` key anywhere; use `docker compose` as the command):
```
HPRADAR_UUID=<cat /proc/sys/kernel/random/uuid>
FEEDER_LAT=20.84
FEEDER_LONG=106.68
FEEDER_ALT_M=10
```

`ULTRAFEEDER_CONFIG` lines the feeder appends:
```
adsb,feed.hpradar.org,30004,beast_reduce_plus_out,uuid=${HPRADAR_UUID};
mlat,feed.hpradar.org,31090,uuid=${HPRADAR_UUID}
```
- `beast_reduce_plus_out` is required to carry the UUID. The plain `beast_reduce_out` connector omits it — do not use that one.
- MLAT also needs `READSB_LAT/LON/ALT` and a station name (`MLAT_USER`), per ultrafeeder MLAT config.

Hand-config (non-Docker) equivalent for readsb:
```
--net-connector=feed.hpradar.org,30004,beast_reduce_plus_out,uuid=<UUID>
```
(Ref: wiedehopf/readsb README.)

### 2.2 AIS client — AIS-catcher

The feeder runs AIS-catcher and pushes to hpradar. Two transports — **prefer HTTP JSON** for clean per-station attribution and auth:

HTTP (recommended):
```
AIS-catcher -d <device> -H https://feed.hpradar.org/ingest/ais \
  id <ingest_token> protocol aiscatcher interval 30 -X <HPRADAR_UUID>
```
- The `aiscatcher` JSON protocol payload includes `stationid`, receiver metadata, raw `nmea`, and decoded fields. (Ref: AIS-catcher HTTP docs.)
- `-X <uuid>` registers/identifies the station (AIS-catcher ≥ v0.58). **[Unverified – confirm]** exact `-X` semantics and whether `id`/key is passed positionally; verify against the installed AIS-catcher version.

UDP fallback (NMEA, no per-message auth):
```
AIS-catcher -d <device> -u feed.hpradar.org 5012
```
Use only for trusted/manually-registered stations, since UDP NMEA carries no clean station credential.

### 2.3 Optional phase-2 client agent [Inference]

A tiny `hpradar-agent` (Go binary) on the box for richer telemetry the data stream alone can't give: heartbeat, box health (CPU/temp/disk), decoder version, and to drive the downtime-alert feature. Optional — the ingest stream already yields `last_seen`. Build only if you want box-health in the dashboard.

---

## 3. Decoder & ingest (server side)

Each transport terminates in an ingest service that **tags every message with its `feed_source.uuid`** before anything else, then writes to the Redis write buffer, then the fusion core. The tag is what makes all per-feeder analytics possible.

### 3.1 ADS-B / Mode-S ingest

Two options for terminating BRP feeds:

**Option A — reuse readsb in ingest mode.**
Run a central readsb with receiver-id ingest enabled:
```
readsb --net --net-only --net-bi-port=30004 --net-receiver-id --net-ingest ...
```
Feeders connect via `beast_reduce_plus_out`; readsb reads the per-connection UUID. (Ref: readsb README — aggregator side enables `--net-receiver-id` and `--net-ingest`.) **[Unverified – confirm]** exact flag spelling and how readsb exposes per-receiver attribution downstream in the installed build.

**Option B (recommended for attribution) — custom Go BRP terminator.**
Terminate feeder TCP connections in your own Go service, parse the Beast stream yourself, read the receiver-id/UUID message, tag frames, decode (or forward decoded positions), emit to Redis. This gives clean, first-class per-feeder attribution without depending on readsb internals — and fits your existing polyglot plan (C/Rust for RF/decode math, Go for ingest/analytics).
- Beast frame parsing: `0x1a` escape framing, types `0x31/0x32/0x33` (Mode-AC / short / long), 6-byte 12 MHz timestamp + signal byte. **[Unverified – confirm]** the receiver-id message type (reported as `0xe3`) and its byte layout against readsb `net_io.c` before implementing; this mapping is the crux of per-feeder stats.
- You can FFI-bind existing C position-decode (track.c) rather than re-deriving CPR math.

### 3.2 MLAT ingest

Run the open-source **mlat-server** (wiedehopf fork). Feeders' `mlat-client` connect on the MLAT port (commonly `31090`) with their UUID + station name; the server computes positions from multi-receiver timing and returns results. Tag MLAT-derived positions as `kind='mlat'` and attribute to the participating feed_sources for "MLAT contribution" credit. **[Unverified – confirm]** mlat-server port and result-feedback wiring for the chosen fork.

### 3.3 AIS ingest

`POST /ingest/ais` (Go):
1. Authenticate via `ingest_token` (or accept provisional, quarantined-trust, for unclaimed UUID).
2. Parse AIS-catcher `aiscatcher` JSON: read `stationid`/UUID, iterate `msgs[]` (raw `nmea` + decoded fields).
3. Tag each message with `feed_source.uuid`, push to Redis, into fusion.
- AIS-catcher already decodes, so the server can validate + tag + store without re-decoding. If you need server-side decode (e.g. UDP NMEA path), the `aiscat` Python bindings or a Go AIS library handle AIVDM. (Ref: AIS-catcher releases — `aiscat` on PyPI.)

UDP NMEA listener (fallback) on a dedicated port maps source → station by pre-registration.

### 3.4 Validation / trust at ingest [Inference]

Run cheaply, per message or per short window, before fusion:
- **Position plausibility:** decoded position consistent with the station's claimed location and physical range geometry. Implausible positions lower `trust_score` and are excluded from leaderboards. This deters spoofed/replayed data; it does not remove the possibility.
- **Rate limiting / caps:** bound each feed_source's write rate in the Redis buffer so one noisy or abusive source does not degrade the fusion core.
- **Duplicate-feed detection:** same data arriving under two UUIDs from correlated timing → flag for review (relevant to perk-farming).

### 3.5 Pipeline

```
[client decoders] --BRP/HTTP/UDP--> [ingest svc: tag w/ feed_source.uuid]
   -> [validation/trust] -> [Redis Stream write buffer]
   -> [fusion core: cross-feeder dedup + position consolidation]
   -> [TimescaleDB: positions hypertable + attribution table]
```

---

## 4. Per-feeder metrics (powers ops + engagement)

Keep a lightweight attribution record, then roll up with TimescaleDB continuous aggregates and prune the raw rows.

```sql
-- raw attribution (short retention, e.g. 7 days, then dropped after rollup)
CREATE TABLE msg_attr (
  ts            timestamptz NOT NULL,
  feed_uuid     uuid NOT NULL,
  target_id     text NOT NULL,      -- ICAO hex or MMSI
  kind          text NOT NULL,      -- adsb|mlat|ais|uat
  range_km      real
);
SELECT create_hypertable('msg_attr','ts');

-- health (1-min buckets)
CREATE MATERIALIZED VIEW feed_health_1m
WITH (timescaledb.continuous) AS
SELECT time_bucket('1 minute', ts) AS bucket,
       feed_uuid,
       count(*)                          AS msgs,
       count(DISTINCT target_id)         AS targets,
       max(range_km)                     AS max_range_km
FROM msg_attr GROUP BY 1,2;

-- daily contribution (unique targets, max range, MLAT share)
CREATE MATERIALIZED VIEW feed_contribution_1d
WITH (timescaledb.continuous) AS
SELECT time_bucket('1 day', ts) AS day,
       feed_uuid,
       count(DISTINCT target_id) AS unique_targets,
       max(range_km)             AS max_range_km,
       count(*) FILTER (WHERE kind='mlat') AS mlat_msgs
FROM msg_attr GROUP BY 1,2;
```

Derived metrics computed from the above:
- **Uptime / streak:** gaps in `last_seen` / per-minute presence.
- **Marginal unique coverage ("only-you"):** targets where, in a time bucket, the count of distinct feeders ≤ N. Compute over fused data:
  ```
  GROUP BY target_id, time_bucket
  HAVING count(DISTINCT feed_uuid) <= N
  ```
  Persist hits to a `unique_catch(feed_uuid, target_id, ts, reason)` table — this is the data behind the strongest engagement hook.
- **Rank by percentile within comparable feeders** (region/density cohort), not raw global volume. (FlightAware ranks by *unique aircraft*, deliberately excluding low-quality Mode-S "Other" — mirror that judgment.)

---

## 5. API surface (for dashboard + agent)

| Method | Path | Purpose |
|---|---|---|
| POST | `/ingest/ais` | AIS-catcher HTTP feed |
| (TCP) | `:30004` | BRP ADS-B ingest |
| (TCP) | `:31090` | MLAT |
| GET | `/api/station/{id}/stats` | live + 30d stats for dashboard |
| GET | `/api/station/{id}/health` | uptime, last_seen, streak |
| GET | `/api/station/{id}/unique` | "only-you" catch feed |
| GET | `/api/leaderboard?cohort=` | percentile ranking |
| POST | `/api/claim` | claim flow (code -> bind account) |

---

## 6. Build sequence (L0–L5)

- **L0 — identity core.** Schema (§1.4), feed-first auto-registration, claim flow, ingest_token issuance.
- **L1 — ADS-B ingest.** BRP terminator (Option B recommended), tag→Redis→fusion→TimescaleDB. A feeder can add one `ULTRAFEEDER_CONFIG` line and appear.
- **L2 — AIS ingest.** `/ingest/ais`, AIS-catcher HTTP path, same tag→fusion flow.
- **L3 — metrics rollups.** `msg_attr` + continuous aggregates + uptime/streak + "only-you" job.
- **L4 — MLAT.** mlat-server integration, MLAT attribution.
- **L5 — validation/trust + perk gating.** Plausibility, rate caps, duplicate-feed detection, perk eligibility on trust + claimed.

The engagement dashboard consumes L3 outputs and is specified separately; L0–L3 are the minimum for a feeder to feed and see their contribution.

---

## 7. Open decisions / confirm before building

1. **[Unverified – confirm]** Beast receiver-id message type/layout (`0xe3`?) in the installed readsb, for Option B. Crux of per-feeder attribution.
2. **[Unverified – confirm]** readsb `--net-receiver-id` / `--net-ingest` flag spelling and per-receiver output, if Option A.
3. **[Unverified – confirm]** AIS-catcher `-X <uuid>` semantics and `id`/key passing for the installed version.
4. **[Unverified – confirm]** mlat-server fork, MLAT port, result-feedback wiring.
5. **Decision:** hostnames/ports for `feed.hpradar.org` (BRP 30004, MLAT 31090, AIS HTTP) — confirm against Oracle Always Free + DNS-indirection layer already chosen for hpradar.
6. **Decision:** per-aggregator UUID issuance vs. accept-any — affects privacy posture and the claim UX.
7. **Decision:** `N` threshold for "only-you" uniqueness, and the cohort definition for percentile ranking.

---

### Upstream references
- readsb (BRP, receiver-id ingest): https://github.com/wiedehopf/readsb
- ultrafeeder (client config, UUID, MLAT): https://github.com/sdr-enthusiasts/docker-adsb-ultrafeeder
- multifeeder / BRP connector syntax: https://github.com/sdr-enthusiasts/docker-multifeeder
- AIS-catcher (HTTP/UDP/TCP output, JSON, -X): https://github.com/jvde-github/AIS-catcher and https://docs.aiscatcher.org
