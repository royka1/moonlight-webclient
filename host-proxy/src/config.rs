use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(name = "moonlight-host-proxy", about = "Moonlight PWA host proxy")]
pub struct Config {
    /// Listen address
    #[arg(long, default_value = "0.0.0.0")]
    pub bind: String,

    /// Listen port (TCP for HTTP/WS, UDP for QUIC/WebTransport)
    #[arg(long, default_value_t = 47999)]
    pub port: u16,

    /// Path to TLS certificate (PEM). Auto-generated if not provided.
    #[arg(long)]
    pub cert: Option<PathBuf>,

    /// Path to TLS private key (PEM). Auto-generated if not provided.
    #[arg(long)]
    pub key: Option<PathBuf>,

    /// Data directory for auto-generated certs
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Enable mDNS broadcasting (_moonlight._tcp.local)
    #[arg(long)]
    pub mdns: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// Maximum concurrent channels per session
    #[arg(long, default_value_t = 64)]
    pub max_channels: u8,

    /// NvHTTP target base URL override
    #[arg(long)]
    pub nvhttp_base: Option<String>,

    /// Path to the PWA static files (dist/) to serve
    #[arg(long)]
    pub www_root: Option<PathBuf>,
}

impl Config {
    /// Returns the data directory, resolving platform defaults if not set.
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone().unwrap_or_else(|| {
            let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
            base.join("moonlight").join("proxy")
        })
    }
}
