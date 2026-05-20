// NvHTTPS endpoints we proxy on behalf of the PWA.
//
// These run over mTLS on port 47984 using the client cert pair we generated
// during pairing + the pinned server cert. The PWA can't make these calls
// itself (Private Network Access, self-signed cert, can't attach client
// identity via fetch), so we do it here.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{info, warn};

use crate::http::AppState;

const NVHTTPS_PORT: u16 = 47984;
const NVHTTP_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// applist
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct HostQuery {
    host: String,
}

#[derive(Serialize)]
pub struct AppEntry {
    id: u32,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "hdrSupported")]
    hdr_supported: Option<bool>,
}

#[derive(Serialize)]
pub struct AppListResponse {
    apps: Vec<AppEntry>,
}

pub async fn applist_handler(
    State(state): State<AppState>,
    Query(q): Query<HostQuery>,
) -> impl IntoResponse {
    match fetch_applist(&state, &q.host).await {
        Ok(apps) => (StatusCode::OK, Json(AppListResponse { apps })).into_response(),
        Err(e) => {
            warn!(host = %q.host, error = %e, "applist failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": format!("{e:#}")})),
            )
                .into_response()
        }
    }
}

async fn fetch_applist(state: &AppState, host: &str) -> anyhow::Result<Vec<AppEntry>> {
    let client = mtls_client(state, host)?;
    let url = format!(
        "https://{host}:{NVHTTPS_PORT}/applist?uniqueid={uid}&uuid={uuid}",
        uid = state.pairing_state.unique_id(),
        uuid = uuid::Uuid::new_v4(),
    );
    info!(%url, "GET applist");
    let body = client.get(&url).send().await?.text().await?;
    parse_applist(&body)
}

fn parse_applist(body: &str) -> anyhow::Result<Vec<AppEntry>> {
    let mut out = Vec::new();
    let mut cursor = body;
    while let Some(idx) = cursor.find("<App>") {
        cursor = &cursor[idx + "<App>".len()..];
        let end = cursor
            .find("</App>")
            .ok_or_else(|| anyhow::anyhow!("malformed applist: missing </App>"))?;
        let entry = &cursor[..end];
        cursor = &cursor[end + "</App>".len()..];

        let id = extract_inner(entry, "ID")
            .and_then(|s| s.trim().parse::<u32>().ok())
            .ok_or_else(|| anyhow::anyhow!("applist entry missing ID"))?;
        let title = extract_inner(entry, "AppTitle").unwrap_or("Unknown").trim();
        let hdr = extract_inner(entry, "IsHdrSupported").map(|v| v.trim() == "1");

        out.push(AppEntry {
            id,
            title: title.to_string(),
            hdr_supported: hdr,
        });
    }
    if out.is_empty() {
        anyhow::bail!("applist response had no <App> entries: {body}");
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// launch / resume
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LaunchRequest {
    host: String,
    #[serde(rename = "appId")]
    app_id: u32,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
    /// "stereo" | "surround51" | "surround71". Defaults to stereo.
    #[serde(default, rename = "audioConfig")]
    audio_config: Option<String>,
    /// Caller-supplied hex of the 16-byte RI key.
    #[serde(rename = "riKeyHex")]
    ri_key_hex: String,
    /// 32-bit signed integer matching what the client will send in the
    /// stream config.
    #[serde(rename = "riKeyId")]
    ri_key_id: i32,
    /// True for resume of an existing session, false for fresh launch.
    #[serde(default)]
    resume: bool,
}

#[derive(Serialize)]
pub struct LaunchResponse {
    /// RTSP session URL we get back from Sunshine (e.g. "rtsp://...").
    #[serde(rename = "rtspSessionUrl")]
    rtsp_session_url: String,
    /// Game session token Sunshine returns. We hand this back to the PWA
    /// for diagnostic purposes.
    #[serde(rename = "gameSession")]
    game_session: String,
    /// appversion / GfeVersion from the host so the PWA can pass them
    /// into LiStartConnection.
    #[serde(rename = "appVersion")]
    app_version: String,
    #[serde(rename = "gfeVersion")]
    gfe_version: String,
}

pub async fn launch_handler(
    State(state): State<AppState>,
    Json(req): Json<LaunchRequest>,
) -> impl IntoResponse {
    match do_launch(&state, &req).await {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => {
            warn!(host = %req.host, app = req.app_id, error = %e, "launch failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": format!("{e:#}")})),
            )
                .into_response()
        }
    }
}

async fn do_launch(state: &AppState, req: &LaunchRequest) -> anyhow::Result<LaunchResponse> {
    let client = mtls_client(state, &req.host)?;

    let (server_info_app_version, server_info_gfe_version) = fetch_serverinfo_versions(state, &req.host).await
        .unwrap_or_else(|e| {
            warn!("serverinfo failed (continuing with empty versions): {e}");
            (String::new(), String::new())
        });

    let surround_info = match req.audio_config.as_deref() {
        Some("surround51") => "65543",   // (5 << 16) | 7  -> 6 channels, mask 0x3f
        Some("surround71") => "65799",   // (7 << 16) | 7  -> 8 channels, mask 0x63f
        _ => "196610",                    // stereo: (3 << 16) | 2 -> 2 channels, mask 0x3
    };

    let endpoint = if req.resume { "resume" } else { "launch" };
    let url = format!(
        "https://{host}:{NVHTTPS_PORT}/{endpoint}?uniqueid={uid}&uuid={uuid}\
         &appid={app_id}&mode={width}x{height}x{fps}\
         &additionalStates=1&sops=1\
         &rikey={ri}&rikeyid={ri_id}\
         &localAudioPlayMode=0&surroundAudioInfo={surround}\
         &remoteControllersBitmap=0&gcmap=0&gcpersist=0",
        host = req.host,
        uid = state.pairing_state.unique_id(),
        uuid = uuid::Uuid::new_v4(),
        app_id = req.app_id,
        width = req.width,
        height = req.height,
        fps = req.fps,
        ri = req.ri_key_hex,
        ri_id = req.ri_key_id,
        surround = surround_info,
    );

    info!(%url, "GET {endpoint}");
    let mut body = client.get(&url).send().await?.text().await?;
    let mut status_code = extract_status_code(&body);
    info!(body_len = body.len(), status_code, "launch response");

    // Sunshine returns 400 "App already running" when something else owns
    // the app. /resume only succeeds at joining if WE were the original
    // client (vanilla Sunshine accepts the HTTP call but its session_raise
    // silently no-ops, then the RTSP TCP gets FINed). Forks like Apollo
    // can actually serve concurrent clients - try /resume there.
    if !req.resume && status_code == 400 {
        let retry_url = url.replacen("/launch?", "/resume?", 1);
        info!(%retry_url, "/launch rejected, retrying as /resume");
        body = client.get(&retry_url).send().await?.text().await?;
        status_code = extract_status_code(&body);
        info!(body_len = body.len(), status_code, "resume response");
        if status_code == 200 {
            warn!(
                "/resume returned 200 but vanilla Sunshine may silently drop the RTSP \
                 connection if another client owns the active session. If streaming \
                 fails with no data, disconnect the other client first or use a \
                 Sunshine fork (Apollo) for multi-client support."
            );
        }
    }

    if status_code != 200 {
        let msg = extract_status_message(&body);
        anyhow::bail!("host rejected {endpoint}: status_code={status_code} ({msg})");
    }

    let rtsp = extract_inner(&body, "sessionUrl0")
        .ok_or_else(|| anyhow::anyhow!("launch response missing sessionUrl0: {body}"))?
        .trim()
        .to_string();
    let game_session = extract_inner(&body, "gamesession")
        .unwrap_or("0")
        .trim()
        .to_string();

    info!(rtsp = %rtsp, "launch ok");

    // The bitrate is configured by the client side via LiStartConnection;
    // Sunshine doesn't need it here. Bitrate field on the URL is ignored by
    // Sunshine but GFE expects it - omit if you only target Sunshine.
    let _ = req.bitrate;

    Ok(LaunchResponse {
        rtsp_session_url: rtsp,
        game_session,
        app_version: server_info_app_version,
        gfe_version: server_info_gfe_version,
    })
}

async fn fetch_serverinfo_versions(state: &AppState, host: &str) -> anyhow::Result<(String, String)> {
    let client = mtls_client(state, host)?;
    let url = format!(
        "https://{host}:{NVHTTPS_PORT}/serverinfo?uniqueid={uid}&uuid={uuid}",
        uid = state.pairing_state.unique_id(),
        uuid = uuid::Uuid::new_v4(),
    );
    let body = client.get(&url).send().await?.text().await?;
    let app_v = extract_inner(&body, "appversion").unwrap_or("").trim().to_string();
    let gfe_v = extract_inner(&body, "GfeVersion").unwrap_or("").trim().to_string();
    Ok((app_v, gfe_v))
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn mtls_client(state: &AppState, address: &str) -> anyhow::Result<reqwest::Client> {
    // reqwest 0.11 with `native-tls` only exposes Identity::from_pkcs12_der,
    // so we bundle our PEM cert + key into a transient PKCS#12 blob.
    use openssl::{pkcs12::Pkcs12, pkey::PKey, x509::X509};

    let cert = X509::from_pem(state.pairing_state.client_cert_pem().as_bytes())?;
    let key = PKey::private_key_from_pem(state.pairing_state.client_key_pem().as_bytes())?;
    let pkcs12 = Pkcs12::builder()
        .name("moonlight-pwa")
        .pkey(&key)
        .cert(&cert)
        .build2("")?;
    let id = reqwest::Identity::from_pkcs12_der(&pkcs12.to_der()?, "")?;

    let server_cert_pem = state.pairing_state.cached_server_cert(address)?;
    let server_cert = reqwest::Certificate::from_pem(server_cert_pem.as_bytes())?;

    Ok(reqwest::Client::builder()
        .identity(id)
        .add_root_certificate(server_cert)
        .danger_accept_invalid_hostnames(true) // self-signed, no SAN
        .timeout(NVHTTP_TIMEOUT)
        .pool_max_idle_per_host(0)
        .build()?)
}

fn extract_inner<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = body.find(&open)? + open.len();
    let e = body[s..].find(&close)? + s;
    Some(&body[s..e])
}

/// Extract an XML attribute value: `<elem foo="bar">`. Sunshine returns
/// `status_code` / `status_message` as attributes on `<root>` rather than
/// child elements, so we need both.
fn extract_attr<'a>(body: &'a str, attr: &str) -> Option<&'a str> {
    let needle = format!("{attr}=\"");
    let s = body.find(&needle)? + needle.len();
    let e = body[s..].find('"')? + s;
    Some(&body[s..e])
}

fn extract_status_code(body: &str) -> i32 {
    extract_inner(body, "status_code")
        .or_else(|| extract_attr(body, "status_code"))
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(200)
}

fn extract_status_message(body: &str) -> &str {
    extract_inner(body, "status_message")
        .or_else(|| extract_attr(body, "status_message"))
        .unwrap_or("(no message)")
}
