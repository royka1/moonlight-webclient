// Five-step NvHTTP pairing handshake against Sunshine / GeForce Experience.
// Driven from a single POST /api/pair endpoint that the PWA calls.
//
// Persists a long-lived client cert + key + unique id under data_dir/.
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use openssl::{
    asn1::Asn1Time,
    bn::{BigNum, MsbOption},
    hash::{Hasher, MessageDigest},
    pkey::{PKey, Private},
    rsa::Rsa,
    sign::{Signer, Verifier},
    symm::{Cipher, Crypter, Mode},
    x509::{X509, X509Builder, X509NameBuilder},
};
use serde::{Deserialize, Serialize};
use std::{path::{Path, PathBuf}, sync::Arc, time::Duration};
use tracing::{info, warn};

const DEVICE_NAME: &str = "moonlight-pwa";
const NVHTTP_PORT: u16 = 47989;
// Step 1 (getservercert) blocks server-side until the user submits the PIN
// in Sunshine's web UI, which can realistically take a minute. Steps 2–5
// then return promptly. Use a generous total cap.
const PAIR_TIMEOUT: Duration = Duration::from_secs(180);
const HASH_BYTES: usize = 32;

#[derive(Clone)]
pub struct PairingState {
    data_dir: PathBuf,
    identity: Arc<ClientIdentity>,
}

impl PairingState {
    pub async fn new(data_dir: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let data_dir = data_dir.into();
        tokio::fs::create_dir_all(&data_dir).await?;
        let identity = Arc::new(ClientIdentity::load_or_create(&data_dir)?);
        Ok(Self { data_dir, identity })
    }

    #[allow(dead_code)]
    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    pub fn client_cert_pem(&self) -> &str {
        &self.identity.cert_pem
    }

    pub fn client_key_pem(&self) -> &str {
        &self.identity.key_pem
    }

    pub fn unique_id(&self) -> &str {
        &self.identity.unique_id
    }

    /// PEM bytes of the server cert we cached during a successful pair with
    /// `address`. Returns Err if not paired (no cached cert on disk).
    pub fn cached_server_cert(&self, address: &str) -> anyhow::Result<String> {
        let path = self
            .data_dir
            .join("hosts")
            .join(format!("{}.cert.pem", sanitize(address)));
        std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("no cached server cert for {address}: {e}"))
    }
}

#[derive(Deserialize)]
pub struct PairRequest {
    address: String,
    pin: String,
    /// What the PWA wants to identify itself as on the host. Shows up in
    /// Sunshine's Paired Clients list. Optional - defaults to "moonlight-pwa"
    /// if the PWA doesn't send it.
    #[serde(rename = "deviceName")]
    #[serde(default)]
    device_name: Option<String>,
}

#[derive(Serialize, Default)]
struct PairResponse {
    paired: bool,
    #[serde(skip_serializing_if = "Option::is_none", rename = "serverCert")]
    server_cert: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "clientCert")]
    client_cert: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "clientKey")]
    client_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub async fn pair_handler(
    State(state): State<PairingState>,
    Json(req): Json<PairRequest>,
) -> impl IntoResponse {
    info!(addr = %req.address, "pair request received");

    let device_name = req
        .device_name
        .as_deref()
        .map(sanitize_device_name)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEVICE_NAME.to_string());

    let result = tokio::time::timeout(
        PAIR_TIMEOUT,
        run_pair(&state.identity, &state.data_dir, &req.address, &req.pin, &device_name),
    )
    .await;

    let body = match result {
        Ok(Ok(server_cert_pem)) => {
            info!(addr = %req.address, "pair succeeded");
            PairResponse {
                paired: true,
                server_cert: Some(server_cert_pem),
                client_cert: Some(state.identity.cert_pem.clone()),
                client_key: Some(state.identity.key_pem.clone()),
                error: None,
            }
        }
        Ok(Err(e)) => {
            warn!(addr = %req.address, error = %e, "pair failed");
            PairResponse {
                paired: false,
                error: Some(format!("{e:#}")),
                ..Default::default()
            }
        }
        Err(_) => {
            warn!(addr = %req.address, "pair timed out");
            PairResponse {
                paired: false,
                error: Some("timed out".into()),
                ..Default::default()
            }
        }
    };

    (StatusCode::OK, Json(body))
}

// ============================================================================
// Client identity
// ============================================================================

struct ClientIdentity {
    unique_id: String,
    cert: X509,
    key: PKey<Private>,
    cert_pem: String,
    key_pem: String,
}

impl ClientIdentity {
    fn load_or_create(dir: &Path) -> anyhow::Result<Self> {
        let cert_path = dir.join("client_cert.pem");
        let key_path = dir.join("client_key.pem");
        let id_path = dir.join("unique_id.txt");

        if !cert_path.exists() || !key_path.exists() {
            generate_x509(&cert_path, &key_path)?;
        }
        if !id_path.exists() {
            std::fs::write(&id_path, hex::encode(rand_bytes(8)))?;
        }

        let cert_pem = std::fs::read_to_string(&cert_path)?;
        let key_pem = std::fs::read_to_string(&key_path)?;
        let cert = X509::from_pem(cert_pem.as_bytes())?;
        let key = PKey::private_key_from_pem(key_pem.as_bytes())?;
        let unique_id = std::fs::read_to_string(&id_path)?.trim().to_string();

        Ok(Self {
            unique_id,
            cert,
            key,
            cert_pem,
            key_pem,
        })
    }

    fn cert_pem_hex(&self) -> String {
        hex::encode(self.cert_pem.as_bytes())
    }

    fn cert_signature(&self) -> Vec<u8> {
        self.cert.signature().as_slice().to_vec()
    }
}

fn generate_x509(cert_path: &PathBuf, key_path: &PathBuf) -> anyhow::Result<()> {
    let rsa = Rsa::generate(2048)?;
    let key = PKey::from_rsa(rsa)?;

    let mut builder = X509Builder::new()?;
    builder.set_version(2)?;
    let mut serial = BigNum::new()?;
    serial.rand(159, MsbOption::MAYBE_ZERO, false)?;
    let serial = serial.to_asn1_integer()?;
    builder.set_serial_number(&serial)?;

    let mut name = X509NameBuilder::new()?;
    name.append_entry_by_text("CN", "NVIDIA GameStream Client")?;
    let name = name.build();
    builder.set_subject_name(&name)?;
    builder.set_issuer_name(&name)?;

    let not_before = Asn1Time::days_from_now(0)?;
    builder.set_not_before(&not_before)?;
    let not_after = Asn1Time::days_from_now(20 * 365)?;
    builder.set_not_after(&not_after)?;
    builder.set_pubkey(&key)?;
    builder.sign(&key, MessageDigest::sha256())?;

    std::fs::write(cert_path, builder.build().to_pem()?)?;
    std::fs::write(key_path, key.private_key_to_pem_pkcs8()?)?;
    Ok(())
}

// ============================================================================
// 4-step handshake
//
// Mirrors moonlight-chrome/libgamestream/pairing.c:gs_pair() byte for byte.
// All four steps are plain HTTP on port 47989. NO uuid parameter (the
// reference uses uniqueid as the session key). NO step 5 over mTLS - that
// was confusion with a different fork.
//
// On any failure, we call /unpair to clear half-pair state on the host so
// the next attempt starts clean.
// ============================================================================

async fn run_pair(
    id: &ClientIdentity,
    data_dir: &Path,
    address: &str,
    pin: &str,
    device_name: &str,
) -> anyhow::Result<String> {
    let result = do_pair(id, address, pin, device_name).await;

    if result.is_err() {
        // Reference behaviour: any failure -> /unpair to wipe partial state.
        let http = reqwest::Client::builder()
            .timeout(PAIR_TIMEOUT)
            .build()?;
        let url = format!(
            "http://{address}:{NVHTTP_PORT}/unpair?uniqueid={uid}",
            uid = id.unique_id,
        );
        if let Err(e) = http.get(&url).send().await {
            tracing::debug!("/unpair after failure: {e}");
        }
    }

    let server_cert_pem = result?;

    // Persist server cert for future NvHTTPS calls.
    let host_dir = data_dir.join("hosts");
    std::fs::create_dir_all(&host_dir).ok();
    std::fs::write(
        host_dir.join(format!("{}.cert.pem", sanitize(address))),
        server_cert_pem.as_bytes(),
    )
    .ok();

    Ok(server_cert_pem)
}

async fn do_pair(
    id: &ClientIdentity,
    address: &str,
    pin: &str,
    device_name: &str,
) -> anyhow::Result<String> {
    let salt = rand_bytes(16);
    let aes_key = derive_aes_key(&salt, pin)?;

    // Sunshine's HTTP server is known to misbehave with keep-alive across
    // pair steps - force a fresh TCP connection per request.
    let http = reqwest::Client::builder()
        .timeout(PAIR_TIMEOUT)
        .pool_max_idle_per_host(0)
        .build()?;

    let base = format!("http://{address}:{NVHTTP_PORT}/pair");

    // ---- Step 1: getservercert ----
    let url = format!(
        "{base}?uniqueid={uid}&devicename={dev}&updateState=1\
         &phrase=getservercert&salt={salt}&clientcert={cert}",
        uid = id.unique_id,
        dev = device_name,
        salt = hex::encode(&salt),
        cert = id.cert_pem_hex(),
    );
    tracing::info!(step = 1, "GET pair?phrase=getservercert");
    let body = http.get(&url).send().await?.text().await?;
    tracing::info!(step = 1, body_len = body.len(), %body, "step 1 response");
    check_paired(&body, "step 1 (getservercert)")?;
    let server_cert_pem =
        String::from_utf8(hex::decode(extract_xml(&body, "plaincert")?)?)?;
    let server_cert = X509::from_pem(server_cert_pem.as_bytes())?;
    let server_cert_sig = server_cert.signature().as_slice().to_vec();
    let server_pubkey = server_cert.public_key()?;

    // ---- Step 2: clientchallenge ----
    let random_challenge = rand_bytes(16);
    let enc_challenge = aes_ecb(&aes_key, &random_challenge, Mode::Encrypt)?;
    let url = format!(
        "{base}?uniqueid={uid}&devicename={dev}&updateState=1\
         &clientchallenge={ch}",
        uid = id.unique_id,
        dev = device_name,
        ch = hex::encode(&enc_challenge),
    );
    tracing::info!(step = 2, "GET pair?clientchallenge");
    let body = http.get(&url).send().await?.text().await?;
    tracing::info!(step = 2, body_len = body.len(), %body, "step 2 response");
    check_paired(&body, "step 2 (clientchallenge)")?;
    let enc_resp = hex::decode(extract_xml(&body, "challengeresponse")?)?;
    let decrypted = aes_ecb(&aes_key, &enc_resp, Mode::Decrypt)?;
    if decrypted.len() < HASH_BYTES + 16 {
        anyhow::bail!("server response too short ({})", decrypted.len());
    }
    let server_response = decrypted[..HASH_BYTES].to_vec();
    let server_challenge = decrypted[HASH_BYTES..HASH_BYTES + 16].to_vec();

    // ---- Step 3: serverchallengeresp ----
    let client_secret = rand_bytes(16);
    let client_response = {
        let mut h = Hasher::new(MessageDigest::sha256())?;
        h.update(&server_challenge)?;
        h.update(&id.cert_signature())?;
        h.update(&client_secret)?;
        h.finish()?.to_vec()
    };
    let enc_client_resp = aes_ecb(&aes_key, &client_response, Mode::Encrypt)?;
    let url = format!(
        "{base}?uniqueid={uid}&devicename={dev}&updateState=1\
         &serverchallengeresp={s}",
        uid = id.unique_id,
        dev = device_name,
        s = hex::encode(&enc_client_resp),
    );
    tracing::info!(step = 3, "GET pair?serverchallengeresp");
    let body = match http.get(&url).send().await {
        Ok(r) => match r.text().await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(step = 3, error = %e, "response read failed");
                return Err(e.into());
            }
        },
        Err(e) => {
            tracing::error!(step = 3, error = %e, "request failed");
            return Err(e.into());
        }
    };
    tracing::info!(step = 3, body_len = body.len(), %body, "step 3 response");
    check_paired(&body, "step 3 (serverchallengeresp)")?;
    let pairing_secret = hex::decode(extract_xml(&body, "pairingsecret")?)?;
    if pairing_secret.len() < 16 + 256 {
        anyhow::bail!("pairingsecret too short ({})", pairing_secret.len());
    }
    let server_secret = &pairing_secret[..16];
    let server_secret_sig = &pairing_secret[16..];

    let mut v = Verifier::new(MessageDigest::sha256(), &server_pubkey)?;
    v.update(server_secret)?;
    if !v.verify(server_secret_sig)? {
        anyhow::bail!("server secret signature invalid - host's identity changed?");
    }

    let expected = {
        let mut h = Hasher::new(MessageDigest::sha256())?;
        h.update(&random_challenge)?;
        h.update(&server_cert_sig)?;
        h.update(server_secret)?;
        h.finish()?.to_vec()
    };
    if expected != server_response {
        anyhow::bail!("server challenge mismatch - wrong PIN entered on the host?");
    }

    // ---- Step 4: clientpairingsecret ----
    let client_secret_sig = {
        let mut s = Signer::new(MessageDigest::sha256(), &id.key)?;
        s.update(&client_secret)?;
        s.sign_to_vec()?
    };
    let mut client_pair_secret = client_secret.clone();
    client_pair_secret.extend_from_slice(&client_secret_sig);
    let url = format!(
        "{base}?uniqueid={uid}&devicename={dev}&updateState=1\
         &clientpairingsecret={s}",
        uid = id.unique_id,
        dev = device_name,
        s = hex::encode(&client_pair_secret),
    );
    tracing::info!(step = 4, "GET pair?clientpairingsecret");
    let body = http.get(&url).send().await?.text().await?;
    tracing::info!(step = 4, body_len = body.len(), %body, "step 4 response");
    check_paired(&body, "step 4 (clientpairingsecret)")?;

    Ok(server_cert_pem)
}

// ============================================================================
// Helpers
// ============================================================================

fn derive_aes_key(salt: &[u8], pin: &str) -> anyhow::Result<Vec<u8>> {
    let mut h = Hasher::new(MessageDigest::sha256())?;
    h.update(salt)?;
    h.update(pin.as_bytes())?;
    Ok(h.finish()?[..16].to_vec())
}

fn aes_ecb(key: &[u8], data: &[u8], mode: Mode) -> anyhow::Result<Vec<u8>> {
    let cipher = Cipher::aes_128_ecb();
    let mut c = Crypter::new(cipher, mode, key, None)?;
    c.pad(false);
    let mut out = vec![0u8; data.len() + cipher.block_size()];
    let n = c.update(data, &mut out)?;
    let m = c.finalize(&mut out[n..])?;
    out.truncate(n + m);
    Ok(out)
}

fn rand_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    openssl::rand::rand_bytes(&mut b).expect("rand_bytes");
    b
}

fn extract_xml(body: &str, tag: &str) -> anyhow::Result<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = body
        .find(&open)
        .ok_or_else(|| anyhow::anyhow!("response missing <{tag}>: {body}"))?;
    let e = body[s..]
        .find(&close)
        .ok_or_else(|| anyhow::anyhow!("response missing </{tag}>: {body}"))?;
    Ok(body[s + open.len()..s + e].trim().to_string())
}

fn check_paired(body: &str, step: &str) -> anyhow::Result<()> {
    let paired = extract_xml(body, "paired").unwrap_or_default();
    if paired != "1" {
        anyhow::bail!("{step}: host responded paired={paired}");
    }
    Ok(())
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Clamp a user-provided device name to characters that won't break the
/// pair URL. Sunshine displays this verbatim in its Paired Clients list,
/// so we keep letters/digits/space/dash/underscore and drop the rest, then
/// replace spaces with `+` for URL safety.
fn sanitize_device_name(s: &str) -> String {
    s.trim()
        .chars()
        .take(64)
        .map(|c| match c {
            c if c.is_ascii_alphanumeric() => c,
            '-' | '_' | '.' => c,
            ' ' => '+',
            _ => '_',
        })
        .collect()
}
