use std::sync::{Arc, RwLock};
use std::net::TcpListener;
use std::thread;

use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod api;
mod beast;
mod client;
mod config;
mod decode;
mod enrichment;
mod feeder;
mod ingest;
mod receiver;
mod reduce;
mod sbs;
mod wire;
mod writer;
mod ws;

use config::Config;
use decode::aircraft::Store;
use feeder::FeederTracker;
use writer::OutputWriter;

fn main() {
    let cfg = Config::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    info!(?cfg, "fa3-v4 starting");

    let tracker = Arc::new(FeederTracker::new());

    let mut beast_out = OutputWriter::new(50, cfg.net_heartbeat);
    let mut garbage_out = cfg.net_garbage_port.map(|_| OutputWriter::new(50, cfg.net_heartbeat));

    let bo_listener = TcpListener::bind(format!("0.0.0.0:{}", cfg.net_bo_port))
        .expect("bind beast output port");
    info!(port = cfg.net_bo_port, "beast output bound");

    let garb_listener = cfg.net_garbage_port.map(|p| {
        let l = TcpListener::bind(format!("0.0.0.0:{}", p)).expect("bind garbage port");
        info!(port = p, "garbage output bound");
        l
    });

    // Reduce output (only if decode enabled)
    let mut reduce_out = if cfg.decode { cfg.net_beast_reduce_out_port.map(|_| OutputWriter::new(50, cfg.net_heartbeat)) } else { None };
    let mut reduce_listener = if cfg.decode {
        cfg.net_beast_reduce_out_port.map(|p| {
            let l = TcpListener::bind(format!("0.0.0.0:{}", p)).expect("bind reduce port");
            info!(port = p, "beast reduce output bound");
            l
        })
    } else { None };
    let mut reduce_state = if cfg.decode {
        cfg.net_beast_reduce_out_port.map(|_| reduce::ReduceState::new(cfg.beast_reduce_interval))
    } else { None };

    // SBS output (only if decode enabled)
    let mut sbs_out = if cfg.decode { cfg.net_sbs_port.map(|_| OutputWriter::new(50, cfg.net_heartbeat)) } else { None };
    let mut sbs_listener = if cfg.decode {
        cfg.net_sbs_port.map(|p| {
            let l = TcpListener::bind(format!("0.0.0.0:{}", p)).expect("bind sbs port");
            info!(port = p, "sbs output bound");
            l
        })
    } else { None };

    // Atlas output (only if decode enabled)
    let mut atlas_out = if cfg.decode { cfg.net_atlas_port.map(|_| OutputWriter::new(50, cfg.net_heartbeat)) } else { None };
    let mut atlas_listener = if cfg.decode {
        cfg.net_atlas_port.map(|p| {
            let l = TcpListener::bind(format!("0.0.0.0:{}", p)).expect("bind atlas port");
            info!(port = p, "atlas output bound");
            l
        })
    } else { None };

    // Decode store + shared caches (only if --decode)
    let mut aircraft_store = if cfg.decode { Some(Store::new("http://localhost:8081")) } else { None };
    let json_cache: Arc<RwLock<Vec<u8>>> = Arc::new(RwLock::new(Vec::new()));
    let bincraft_cache: Arc<RwLock<Vec<u8>>> = Arc::new(RwLock::new(Vec::new()));
    let trace_cache: Arc<RwLock<std::collections::HashMap<u32, String>>> = Arc::new(RwLock::new(std::collections::HashMap::new()));

    // Spawn HTTP API
    let api_tracker = Arc::clone(&tracker);
    let http_addr = cfg.http.clone();
    let api_json = Arc::clone(&json_cache);
    let api_bc = Arc::clone(&bincraft_cache);
    let api_tc = Arc::clone(&trace_cache);
    let web_dir = cfg.web_dir.clone();
    let decode_enabled = cfg.decode;
    thread::spawn(move || {
        api::run_blocking(api_tracker, &http_addr, api_json, api_bc, api_tc, web_dir, decode_enabled);
    });

    // Spawn WebSocket push server
    if let Some(ws_port) = cfg.ws_port {
        if cfg.decode {
            let ws_cache = Arc::clone(&bincraft_cache);
            let ws_addr = format!("0.0.0.0:{}", ws_port);
            thread::spawn(move || ws::run_ws_server(&ws_addr, ws_cache));
        }
    }

    // Run single-threaded ingest loop (blocks forever)
    ingest::run_ingest_loop(
        cfg.net_bi_port,
        &mut beast_out,
        &mut garbage_out,
        &tracker,
        cfg.net_receiver_id,
        &mut Some(bo_listener),
        &mut garb_listener.map(Some).unwrap_or(None),
        &mut aircraft_store,
        json_cache,
        bincraft_cache,
        trace_cache,
        &mut reduce_out,
        &mut reduce_listener,
        &mut reduce_state,
        &mut sbs_out,
        &mut sbs_listener,
        &mut atlas_out,
        &mut atlas_listener,
    );
}
