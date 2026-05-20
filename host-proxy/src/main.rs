mod channel;
mod config;
mod frame;
mod http;
mod mdns;
mod nvhttp;
mod pairing;
mod tcp_relay;
mod tls;
mod udp_relay;
mod ws_session;
mod wt_session;

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::config::Config;
use crate::http::{build_router, AppState};
use crate::pairing::PairingState;
use crate::tls::load_or_generate;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::parse();

    // Init tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.log_level)),
        )
        .init();

    info!(
        "moonlight-host-proxy v{} starting",
        env!("CARGO_PKG_VERSION")
    );
    info!("Listening on {}:{}", config.bind, config.port);

    // Load or generate TLS certificate.
    let tls_material = load_or_generate(&config)?;

    let cancel = CancellationToken::new();

    // Start mDNS if enabled.
    if config.mdns {
        mdns::start(config.port, cancel.child_token());
    }

    // Load or create the pairing identity (persistent X.509 cert+key+unique_id).
    let pairing_state = PairingState::new(config.data_dir()).await?;

    // Build shared state for axum.
    let app_state = AppState {
        config: Arc::new(config.clone()),
        cancel: cancel.child_token(),
        pairing_state,
    };

    // PWA static-file resolution order:
    //   1. --www-root <path>                 (explicit override)
    //   2. ./dist next to the running binary (handy for dev / portable installs)
    //   3. ./dist next to the current working directory
    //   4. Embedded PWA bundle compiled into the binary
    //
    // (4) means the binary is fully self-contained — running it with no
    // arguments serves a working PWA.
    let www_root = config
        .www_root
        .clone()
        .or_else(dist_next_to_binary)
        .or_else(dist_in_cwd);
    info!(
        "Serving PWA from {}",
        match &www_root {
            Some(p) => format!("disk: {}", p.display()),
            None => "embedded bundle (compiled into binary)".into(),
        }
    );

    let router = build_router(app_state, www_root);

    // Build rustls config.
    let mut tls_cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            tls_material.cert_chain.clone(),
            clone_private_key(&tls_material.key),
        )
        .expect("invalid TLS config");

    tls_cfg.alpn_protocols = vec![b"http/1.1".to_vec(), b"h2".to_vec()];
    let tls_cfg = Arc::new(tls_cfg);

    // TCP/HTTPS listener.
    let listener =
        tokio::net::TcpListener::bind(format!("{}:{}", config.bind, config.port)).await?;

    let tls_server_config = axum_server::tls_rustls::RustlsConfig::from_config(tls_cfg.clone());

    info!(
        "TCP/TLS server listening on {}",
        listener.local_addr().unwrap()
    );

    let tcp_cancel = cancel.clone();
    let server = axum_server::from_tcp_rustls(listener.into_std()?, tls_server_config)
        .map_err(|e| anyhow::anyhow!("failed to create TLS server: {e}"))?;

    // Graceful shutdown on ctrl-c.
    tokio::select! {
        result = server.serve(router.into_make_service()) => {
            if let Err(e) = result {
                error!("TCP server error: {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Shutting down...");
        }
        _ = tcp_cancel.cancelled() => {
            info!("Shutting down via cancel...");
        }
    }

    cancel.cancel();
    info!("Proxy stopped.");

    Ok(())
}

fn dist_next_to_binary() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let dist = dir.join("dist");
    if dist.is_dir() { Some(dist) } else { None }
}

fn dist_in_cwd() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let dist = cwd.join("dist");
    if dist.is_dir() { Some(dist) } else { None }
}

fn clone_private_key(
    key: &rustls::pki_types::PrivateKeyDer<'_>,
) -> rustls::pki_types::PrivateKeyDer<'static> {
    use rustls::pki_types::{
        PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
    };
    match key {
        PrivateKeyDer::Pkcs8(k) => {
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(k.secret_pkcs8_der().to_vec()))
        }
        PrivateKeyDer::Sec1(k) => {
            PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(k.secret_sec1_der().to_vec()))
        }
        PrivateKeyDer::Pkcs1(k) => {
            PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(k.secret_pkcs1_der().to_vec()))
        }
        _ => unreachable!("unexpected private key type"),
    }
}
