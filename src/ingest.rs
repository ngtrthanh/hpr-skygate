use std::collections::HashMap;
use std::io::Read;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use mio::net::TcpListener;
use mio::{Events, Interest, Poll, Token};

use crate::client::ClientState;
use crate::feeder::FeederTracker;
use crate::writer::OutputWriter;

const ACCEPT_TOKEN: Token = Token(0);
const BASE_TOKEN: Token = Token(1);
const BUF_SIZE: usize = 32 * 1024;

struct Feeder {
    stream: mio::net::TcpStream,
    addr: SocketAddr,
    state: ClientState,
}

/// Single-threaded epoll loop: reads all feeders, frames, appends to writers.
pub fn run_ingest_loop(
    bi_port: u16,
    beast_out: &mut OutputWriter,
    garbage_out: &mut Option<OutputWriter>,
    tracker: &FeederTracker,
    prepend_recv_id: bool,
    bo_listener: &mut Option<std::net::TcpListener>,
    garb_listener: &mut Option<std::net::TcpListener>,
    aircraft_store: &mut Option<crate::decode::aircraft::Store>,
    json_cache: std::sync::Arc<std::sync::RwLock<Vec<u8>>>,
    bincraft_cache: std::sync::Arc<std::sync::RwLock<Vec<u8>>>,
    trace_cache: std::sync::Arc<std::sync::RwLock<std::collections::HashMap<u32, String>>>,
    reduce_out: &mut Option<OutputWriter>,
    reduce_listener: &mut Option<std::net::TcpListener>,
    reduce_state: &mut Option<crate::reduce::ReduceState>,
    sbs_out: &mut Option<OutputWriter>,
    sbs_listener: &mut Option<std::net::TcpListener>,
    atlas_out: &mut Option<OutputWriter>,
    atlas_listener: &mut Option<std::net::TcpListener>,
) {
    let mut poll = Poll::new().expect("poll");
    let mut events = Events::with_capacity(4096);
    let mut last_cache_rebuild = Instant::now();
    let mut last_trace_rebuild = Instant::now();

    // Bind beast-in listener
    let addr: SocketAddr = format!("0.0.0.0:{}", bi_port).parse().unwrap();
    let mut listener = TcpListener::bind(addr).expect("bind bi_port");
    poll.registry().register(&mut listener, ACCEPT_TOKEN, Interest::READABLE).unwrap();

    // Tokens for output accept
    let bo_token = Token(usize::MAX - 1);
    let garb_token = Token(usize::MAX - 2);
    let reduce_token = Token(usize::MAX - 3);
    let sbs_token = Token(usize::MAX - 4);
    let atlas_token = Token(usize::MAX - 5);

    // Register output listeners with mio
    let mut bo_mio: Option<TcpListener> = bo_listener.take().map(|l| {
        l.set_nonblocking(true).unwrap();
        let mut ml = TcpListener::from_std(l);
        poll.registry().register(&mut ml, bo_token, Interest::READABLE).unwrap();
        ml
    });
    let mut garb_mio: Option<TcpListener> = garb_listener.take().map(|l| {
        l.set_nonblocking(true).unwrap();
        let mut ml = TcpListener::from_std(l);
        poll.registry().register(&mut ml, garb_token, Interest::READABLE).unwrap();
        ml
    });
    let mut reduce_mio: Option<TcpListener> = reduce_listener.take().map(|l| {
        l.set_nonblocking(true).unwrap();
        let mut ml = TcpListener::from_std(l);
        poll.registry().register(&mut ml, reduce_token, Interest::READABLE).unwrap();
        ml
    });
    let mut sbs_mio: Option<TcpListener> = sbs_listener.take().map(|l| {
        l.set_nonblocking(true).unwrap();
        let mut ml = TcpListener::from_std(l);
        poll.registry().register(&mut ml, sbs_token, Interest::READABLE).unwrap();
        ml
    });
    let mut atlas_mio: Option<TcpListener> = atlas_listener.take().map(|l| {
        l.set_nonblocking(true).unwrap();
        let mut ml = TcpListener::from_std(l);
        poll.registry().register(&mut ml, atlas_token, Interest::READABLE).unwrap();
        ml
    });

    let mut feeders: HashMap<Token, Feeder> = HashMap::new();
    let mut next_token: usize = BASE_TOKEN.0;
    let mut buf = [0u8; BUF_SIZE];

    let flush_check_interval = Duration::from_millis(50);
    let mut last_flush_check = Instant::now();

    // Vec to collect decoded frames for reduce/sbs processing outside the closure
    let mut decoded_frames: Vec<(Vec<u8>, crate::decode::mode_s::ModeS)> = Vec::new();

    tracing::info!(port = bi_port, "ingest loop started");

    loop {
        let timeout = Duration::from_millis(10);
        poll.poll(&mut events, Some(timeout)).unwrap();

        let now = Instant::now();

        for event in events.iter() {
            match event.token() {
                ACCEPT_TOKEN => {
                    loop {
                        match listener.accept() {
                            Ok((mut stream, addr)) => {
                                let addr_str = addr.to_string();
                                if tracker.is_blocked(&addr_str) {
                                    drop(stream);
                                    continue;
                                }
                                next_token += 1;
                                let token = Token(next_token);
                                poll.registry().register(&mut stream, token, Interest::READABLE).unwrap();
                                tracker.connect(&addr_str);
                                feeders.insert(token, Feeder {
                                    stream,
                                    addr,
                                    state: ClientState::new(addr_str),
                                });
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => break,
                        }
                    }
                }
                t if t == bo_token => {
                    accept_subscribers(&mut bo_mio, beast_out, "beast");
                }
                t if t == garb_token => {
                    if let Some(ref mut gw) = garbage_out {
                        accept_subscribers(&mut garb_mio, gw, "garbage");
                    }
                }
                t if t == reduce_token => {
                    if let Some(ref mut rw) = reduce_out {
                        accept_subscribers(&mut reduce_mio, rw, "reduce");
                    }
                }
                t if t == sbs_token => {
                    if let Some(ref mut sw) = sbs_out {
                        accept_subscribers(&mut sbs_mio, sw, "sbs");
                    }
                }
                t if t == atlas_token => {
                    if let Some(ref mut aw) = atlas_out {
                        accept_subscribers(&mut atlas_mio, aw, "atlas");
                    }
                }
                token => {
                    let mut remove = false;
                    if let Some(feeder) = feeders.get_mut(&token) {
                        match feeder.stream.read(&mut buf) {
                            Ok(0) => remove = true,
                            Ok(n) => {
                                feeder.state.feed(&buf[..n], prepend_recv_id, |frame, is_garbage| {
                                    if is_garbage {
                                        if let Some(ref mut gw) = garbage_out {
                                            gw.append(frame);
                                        }
                                    } else {
                                        beast_out.append(frame);
                                    }
                                    // Decode if enabled
                                    if aircraft_store.is_some() && frame.len() >= 9 && (frame[1] == 0x32 || frame[1] == 0x33) {
                                        if let Some(payload) = unescape_payload(frame) {
                                            if let Some(msg) = crate::decode::mode_s::decode(&payload) {
                                                aircraft_store.as_mut().unwrap().update(msg.clone());
                                                decoded_frames.push((frame.to_vec(), msg));
                                            }
                                        }
                                    }
                                });
                                tracker.update(&feeder.state.addr, &feeder.state);
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(_) => remove = true,
                        }
                    }
                    if remove {
                        if let Some(mut feeder) = feeders.remove(&token) {
                            poll.registry().deregister(&mut feeder.stream).ok();
                            tracker.disconnect(&feeder.state.addr);
                        }
                    }
                }
            }
        }

        // Process decoded frames for reduce/sbs outside the closure
        if !decoded_frames.is_empty() {
            let emit_now = Instant::now();
            for (raw_frame, msg) in decoded_frames.drain(..) {
                if let Some(ref mut rs) = reduce_state {
                    if rs.should_emit(msg.icao, emit_now) {
                        if let Some(ref mut rw) = reduce_out { rw.append(&raw_frame); }
                    }
                }
                if let Some(ref mut sw) = sbs_out {
                    if let Some(ref store) = aircraft_store {
                        if let Some(ac) = store.map.get(&msg.icao) {
                            if let Some(line) = crate::sbs::format_sbs(&msg, ac) {
                                sw.append(&line);
                            }
                        }
                    }
                }
            }
        }

        // Timer-based flush check
        if now.duration_since(last_flush_check) >= flush_check_interval {
            beast_out.check_flush();
            if let Some(ref mut gw) = garbage_out { gw.check_flush(); }
            if let Some(ref mut rw) = reduce_out { rw.check_flush(); }
            if let Some(ref mut sw) = sbs_out { sw.check_flush(); }
            if let Some(ref mut aw) = atlas_out { aw.check_flush(); }
            if let Some(ref mut store) = aircraft_store { store.reap_stale(); }
            // Rebuild caches + atlas every 1s
            if now.duration_since(last_cache_rebuild) >= Duration::from_secs(1) {
                if let Some(ref store) = aircraft_store {
                    let j = crate::decode::output::build_json(store, None);
                    *json_cache.write().unwrap() = j;
                    let b = crate::decode::output::build_bincraft(store, None);
                    *bincraft_cache.write().unwrap() = b;
                    // Atlas: emit position + static frames for all aircraft
                    if let Some(ref mut aw) = atlas_out {
                        for ac in store.map.values() {
                            if let Some(pos) = crate::wire::encode_position(ac) {
                                aw.append(&pos);
                            }
                            if let Some(st) = crate::wire::encode_static(ac) {
                                aw.append(&st);
                            }
                        }
                    }
                }
                last_cache_rebuild = now;
            }
            // Rebuild trace cache every 5s
            if now.duration_since(last_trace_rebuild) >= Duration::from_secs(5) {
                if let Some(ref store) = aircraft_store {
                    let mut tc = trace_cache.write().unwrap();
                    tc.clear();
                    for (icao, ac) in &store.map {
                        if ac.trace.len() > 1 {
                            tc.insert(*icao, format_trace(ac));
                        }
                    }
                }
                last_trace_rebuild = now;
            }
            last_flush_check = now;
        }
    }
}

fn accept_subscribers(listener: &mut Option<TcpListener>, writer: &mut OutputWriter, label: &str) {
    if let Some(ref mut l) = listener {
        loop {
            match l.accept() {
                Ok((stream, addr)) => {
                    let std_stream = unsafe {
                        use std::os::fd::{FromRawFd, IntoRawFd};
                        std::net::TcpStream::from_raw_fd(stream.into_raw_fd())
                    };
                    writer.add_client(std_stream, addr);
                    tracing::info!(%addr, label, "subscriber connected");
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }
}

/// Unescape beast wire frame, return raw Mode-S payload (7 or 14 bytes).
fn unescape_payload(wire: &[u8]) -> Option<Vec<u8>> {
    if wire.len() < 4 { return None; }
    let payload_len: usize = match wire[1] {
        0x31 => 2, 0x32 => 7, 0x33 => 14, _ => return None,
    };
    let total = 6 + 1 + payload_len;
    let mut out = Vec::with_capacity(payload_len);
    let mut i = 2;
    let mut count = 0;
    while count < total && i < wire.len() {
        if wire[i] == 0x1a {
            i += 1;
            if i >= wire.len() { return None; }
        }
        if count >= 7 { out.push(wire[i]); }
        count += 1;
        i += 1;
    }
    if out.len() == payload_len { Some(out) } else { None }
}

fn format_trace(ac: &crate::decode::aircraft::Aircraft) -> String {
    use std::fmt::Write;
    let mut s = String::from("[");
    for (i, p) in ac.trace.iter().enumerate() {
        if i > 0 { s.push(','); }
        let _ = write!(s, "[{:.1},{:.6},{:.6},{},{}]",
            p.ts, p.lat, p.lon,
            p.alt.map(|a| a.to_string()).unwrap_or_else(|| "null".into()),
            p.gs.map(|g| format!("{:.1}", g)).unwrap_or_else(|| "null".into()),
        );
    }
    s.push(']');
    s
}
