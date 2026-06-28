use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "fa3", about = "Beast ingest fan-out with receiverId tracking")]
pub struct Config {
    /// Beast input listen port (feeders connect here)
    #[arg(long, default_value = "30004")]
    pub net_bi_port: u16,

    /// Beast output listen port (subscribers connect here)
    #[arg(long, default_value = "30005")]
    pub net_bo_port: u16,

    /// Garbage output port (rate-limited feeders, optional)
    #[arg(long)]
    pub net_garbage_port: Option<u16>,

    /// Prepend 0xe3 receiverId to output frames
    #[arg(long, default_value_t = true)]
    pub net_receiver_id: bool,

    /// Enable ingest mode (rate limiting)
    #[arg(long, default_value_t = true)]
    pub net_ingest: bool,

    /// Heartbeat interval in seconds
    #[arg(long, default_value_t = 60)]
    pub net_heartbeat: u64,

    /// HTTP API/dashboard listen address
    #[arg(long, default_value = "0.0.0.0:9876")]
    pub http: String,

    /// Broadcast channel capacity
    #[arg(long, default_value_t = 8192)]
    pub buffer_frames: usize,

    /// Enable Mode-S decode + aircraft tracking
    #[arg(long, default_value_t = false)]
    pub decode: bool,

    /// Serve static files from this directory
    #[arg(long)]
    pub web_dir: Option<String>,

    /// Beast reduce output port (rate-limited per ICAO)
    #[arg(long)]
    pub net_beast_reduce_out_port: Option<u16>,

    /// Beast reduce interval in seconds
    #[arg(long, default_value_t = 0.25)]
    pub beast_reduce_interval: f64,

    /// SBS (BaseStation) output port
    #[arg(long)]
    pub net_sbs_port: Option<u16>,

    /// Atlas binary wire format output port
    #[arg(long)]
    pub net_atlas_port: Option<u16>,

    /// WebSocket push port (binary aircraft data)
    #[arg(long)]
    pub ws_port: Option<u16>,

    /// Compact frame ingest port (from hpr-demod)
    #[arg(long)]
    pub net_demod_port: Option<u16>,

    /// AIS TCP sources (format: "name=host:port,name2=host2:port2")
    #[arg(long)]
    pub ais_sources: Option<String>,

    /// RTL-SDR device index for 1090 MHz ADS-B demod
    #[arg(long)]
    pub sdr_device: Option<u32>,

    /// RTL-SDR device index for 162 MHz AIS demod
    #[arg(long)]
    pub ais_device: Option<u32>,

    /// Receiver latitude
    #[arg(long)]
    pub lat: Option<f64>,

    /// Receiver longitude
    #[arg(long)]
    pub lon: Option<f64>,
}

