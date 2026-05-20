use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, FromRef, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use rust_embed::RustEmbed;
use reqwest::Client;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};
use tracing::{debug, warn};

use crate::config::Config;
use crate::nvhttp;
use crate::pairing::{self, PairingState};
use crate::ws_session;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub cancel: CancellationToken,
    pub pairing_state: PairingState,
}

impl FromRef<AppState> for PairingState {
    fn from_ref(state: &AppState) -> Self {
        state.pairing_state.clone()
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// NvHTTP pass-through: proxy requests to the gaming host's NvHTTP endpoint.
async fn nvhttp_handler(
    method: axum::http::Method,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response<Body>, StatusCode> {
    let target_url = params.get("url").ok_or_else(|| {
        warn!("nvhttp: missing url param");
        StatusCode::BAD_REQUEST
    })?;

    let parsed = url::Url::parse(target_url).map_err(|e| {
        warn!("nvhttp: invalid url '{target_url}': {e}");
        StatusCode::BAD_REQUEST
    })?;

    match parsed.scheme() {
        "http" | "https" => {}
        _ => {
            warn!("nvhttp: disallowed scheme: {}", parsed.scheme());
            return Err(StatusCode::FORBIDDEN);
        }
    }

    let host = parsed.host_str().unwrap_or("");
    if !is_safe_target(host) {
        warn!("nvhttp: target not allowed: {host}");
        return Err(StatusCode::FORBIDDEN);
    }

    debug!("nvhttp: forwarding {method} {target_url}");

    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let reqwest_method = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut req = client.request(reqwest_method, target_url.as_str());

    for (name, value) in headers.iter() {
        let name_str = name.as_str().to_lowercase();
        if is_hop_by_hop(&name_str) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            if let Ok(rv) = reqwest::header::HeaderValue::from_str(v) {
                req = req.header(name.as_str(), rv);
            }
        }
    }

    if !body.is_empty() {
        req = req.body(body.to_vec());
    }

    let resp = req.send().await.map_err(|e| {
        warn!("nvhttp: upstream error: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let status = resp.status();
    let axum_status =
        StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let resp_headers = resp.headers().clone();
    let resp_body = resp.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;

    let mut builder = Response::builder().status(axum_status);
    for (name, value) in resp_headers.iter() {
        let name_str = name.as_str().to_lowercase();
        if is_hop_by_hop(&name_str) {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }

    builder
        .body(Body::from(resp_body.to_vec()))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// WebSocket upgrade handler.
async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        ws_session::run(socket, (*state.config).clone(), state.cancel.child_token()).await;
    })
}

/// Build the axum Router with shared AppState.
///
/// When `www_root` is Some, the proxy also serves the built PWA static files
/// with the required COOP/COEP headers. This lets ChromeOS users install the
/// PWA directly from the proxy's HTTPS origin — same origin = no CORS pain,
/// the wss:// proxy URL is same-origin, and SharedArrayBuffer works.
pub fn build_router(state: AppState, www_root: Option<PathBuf>) -> Router {
    let coop_layer = SetResponseHeaderLayer::overriding(
        axum::http::HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    let coep_layer = SetResponseHeaderLayer::overriding(
        axum::http::HeaderName::from_static("cross-origin-embedder-policy"),
        HeaderValue::from_static("require-corp"),
    );
    let corp_layer = SetResponseHeaderLayer::overriding(
        axum::http::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );

    // API routes (take priority over static files).
    let api = Router::new()
        .route("/health", get(health_handler))
        .route(
            "/api/nvhttp",
            get(nvhttp_handler)
                .post(nvhttp_handler)
                .put(nvhttp_handler),
        )
        .route("/api/pair", post(pairing::pair_handler))
        .route("/api/applist", get(nvhttp::applist_handler))
        .route("/api/launch", post(nvhttp::launch_handler))
        .route("/proxy", get(ws_upgrade_handler))
        .with_state(state);

    if let Some(root) = www_root {
        let index_html = root.join("index.html");

        // ServeDir for static assets. When a path doesn't match any file
        // on disk, fall back to index.html so the SPA's client-side router
        // can handle it (history API fallback).
        let static_files = ServeDir::new(&root)
            .precompressed_gzip()
            .precompressed_br()
            .fallback(ServeFile::new(&index_html));

        // API routes win; everything else falls through to static files.
        api.fallback_service(static_files)
            .layer(corp_layer)
            .layer(coop_layer)
            .layer(coep_layer)
    } else {
        // No on-disk www-root: serve the PWA from the embedded bundle so
        // the binary is fully self-contained.
        api.fallback(serve_embedded)
            .layer(corp_layer)
            .layer(coop_layer)
            .layer(coep_layer)
    }
}

// -------- embedded PWA --------

#[derive(RustEmbed)]
#[folder = "../dist/"]
struct WebAssets;

/// Fallback route: serve the PWA from compiled-in bytes. Unknown paths fall
/// back to index.html so the SPA's client-side router can handle them
/// (same behaviour as the on-disk ServeDir path).
async fn serve_embedded(uri: Uri) -> Response<Body> {
    let raw = uri.path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };

    let (asset, served_path): (rust_embed::EmbeddedFile, &str) = match WebAssets::get(path) {
        Some(a) => (a, path),
        None => match WebAssets::get("index.html") {
            Some(a) => (a, "index.html"),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    "PWA bundle missing from binary (compiled with empty ../dist?)",
                )
                    .into_response();
            }
        },
    };

    let mime = mime_guess::from_path(served_path)
        .first_or_octet_stream()
        .to_string();

    let mut resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(
            header::CACHE_CONTROL,
            // index.html should never be cached; everything else is hashed.
            if served_path == "index.html" {
                "no-store"
            } else {
                "public, max-age=31536000, immutable"
            },
        );
    if let Some(etag) = format_etag(&asset.metadata.sha256_hash()) {
        resp = resp.header(header::ETAG, etag);
    }
    resp.body(Body::from(asset.data.into_owned()))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "embed: response build failed").into_response()
        })
}

fn format_etag(hash: &[u8; 32]) -> Option<String> {
    let mut s = String::with_capacity(2 + 64);
    s.push('"');
    for b in hash {
        s.push_str(&format!("{:02x}", b));
    }
    s.push('"');
    Some(s)
}

fn is_safe_target(host: &str) -> bool {
    if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "0.0.0.0" {
        return true;
    }

    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        return ip.is_private() || ip.is_loopback();
    }

    if let Ok(ip) = host.parse::<std::net::Ipv6Addr>() {
        let octets = ip.octets();
        return ip.is_loopback()
            || (octets[0] & 0xfe) == 0xfc
            || (octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80);
    }

    false
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}
