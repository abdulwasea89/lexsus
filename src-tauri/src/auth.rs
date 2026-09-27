//! The connector's secure layer: bearer-token authentication with optional
//! request signing and always-on response signing.
//!
//! The MCP endpoint can read the workspace and — with the write gate open —
//! change it. Below the transport the bridge already has a full policy engine
//! (approvals, containment, sensitive paths, grants, kill switch, audit), but
//! the transport itself authenticates nobody. That is only sound while the
//! endpoint is genuinely loopback: the moment a tunnel host is opted into via
//! `LEXSUS_MCP_ALLOWED_HOSTS` — the documented path to a hosted connector —
//! anyone who learns the URL has the whole surface.
//!
//! So every request carries a bearer token. It is the one credential every MCP
//! host can actually send: `Authorization` is the header the spec standardises
//! on, and the CLI hosts expose a flag for it.
//!
//! # Why signing is optional on requests but always on responses
//!
//! A per-request HMAC must be recomputed over that request's own body, and no
//! MCP client does that — the ecosystem is bearer- and OAuth-based, with
//! custom-header support only partial. Requiring a signature would answer 401
//! to every stock host, leaving a secured endpoint nothing can talk to. So a
//! *presented* signature is verified strictly (with its nonce and timestamp),
//! and an absent one leaves the bearer carrying the request.
//!
//! The response direction has no such constraint — the server controls it — so
//! every response body is signed. First-party tooling can verify that; the
//! scheme is exposed here (`sign_request`/`verify_response`) precisely so it
//! can.

use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

/// Entropy behind a generated token. 256 bits, so guessing is not a strategy.
const TOKEN_BYTES: usize = 32;
/// Request header carrying the bearer token.
const AUTHORIZATION: &str = "authorization";
/// The three headers a signing caller sends.
pub const NONCE_HEADER: &str = "x-lexsus-nonce";
pub const TIMESTAMP_HEADER: &str = "x-lexsus-timestamp";
pub const SIGNATURE_HEADER: &str = "x-lexsus-signature";
/// How far a signed request's timestamp may be from now, in seconds.
pub const DEFAULT_TTL_SECS: u64 = 300;
/// Ceiling on remembered nonces, so the replay cache cannot grow without bound.
const NONCE_CACHE_CAP: usize = 4096;
/// Domain separator for the signing key, so the key is never the token itself.
const HMAC_DOMAIN: &[u8] = b"lexsus-mcp-hmac-v1";
const KEYRING_SERVICE: &str = "lexsus-mcp";
const KEYRING_USER: &str = "auth-token";
const TOKEN_FILE: &str = "mcp-auth-token";

/// Where the token came from. Reported to the UI so the desktop can say which
/// store is actually in play rather than assuming the keyring worked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// `LEXSUS_MCP_AUTH_TOKEN` — an explicit override, and authoritative.
    Env,
    /// The platform credential store (Secret Service, Keychain, Credential Manager).
    Keyring,
    /// `<app_data_dir>/mcp-auth-token`, mode 0600.
    File,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Env => "env",
            Backend::Keyring => "keyring",
            Backend::File => "file",
        }
    }
}

/// Why a request was refused. Every variant maps to a `401`; the distinction
/// exists for the server log, and for the caller only once their bearer has
/// already checked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    MissingCredentials,
    BadToken,
    MissingSignatureParts,
    /// Carries the window that was enforced, so the message cannot claim the
    /// default when `LEXSUS_MCP_SIGNATURE_TTL_SECS` set something else.
    StaleTimestamp(u64),
    ReplayedNonce,
    BadSignature,
    /// Rotation is meaningless against an environment override: the variable
    /// would win again on the next launch, silently undoing it.
    EnvOverride,
    Persist(String),
    Rng(String),
}

impl AuthError {
    /// A stable machine-readable reason, for the log and for a caller who has
    /// already proven their bearer token.
    pub fn code(&self) -> &'static str {
        match self {
            AuthError::MissingCredentials => "missing_credentials",
            AuthError::BadToken => "bad_token",
            AuthError::MissingSignatureParts => "missing_signature_parts",
            AuthError::StaleTimestamp(_) => "stale_timestamp",
            AuthError::ReplayedNonce => "replayed_nonce",
            AuthError::BadSignature => "bad_signature",
            AuthError::EnvOverride => "env_override",
            AuthError::Persist(_) => "persist_failed",
            AuthError::Rng(_) => "rng_failed",
        }
    }

    /// Human sentence, for the desktop and for an authenticated caller.
    pub fn message(&self) -> String {
        match self {
            AuthError::MissingCredentials => {
                "missing Authorization: Bearer <token> header".to_string()
            }
            AuthError::BadToken => "invalid bearer token".to_string(),
            AuthError::MissingSignatureParts => format!(
                "a {SIGNATURE_HEADER} header needs {TIMESTAMP_HEADER} and {NONCE_HEADER} too"
            ),
            AuthError::StaleTimestamp(ttl) => {
                format!("{TIMESTAMP_HEADER} is outside the {ttl}s freshness window")
            }
            AuthError::ReplayedNonce => {
                format!("{NONCE_HEADER} has already been used")
            }
            AuthError::BadSignature => format!("{SIGNATURE_HEADER} does not match the body"),
            AuthError::EnvOverride => {
                "the token comes from LEXSUS_MCP_AUTH_TOKEN; rotate the variable instead"
                    .to_string()
            }
            AuthError::Persist(e) => format!("could not persist the new token: {e}"),
            AuthError::Rng(e) => format!("the OS random source failed: {e}"),
        }
    }
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for AuthError {}

/// The secret material, wiped on drop.
///
/// `token` is what a client presents; `hmac_key` is derived from it and never
/// equals it. Keeping them distinct matters because a token leaks far more
/// easily than a stored key does — into a header, a log, a screenshot — and
/// reusing one value for both would mean a leaked token also forges signatures.
struct Secret {
    token: Zeroizing<String>,
    hmac_key: Zeroizing<Vec<u8>>,
}

impl Secret {
    /// Derive a secret from a token value, however it was obtained.
    fn from_token(token: &str) -> Self {
        let mut h = Sha256::new();
        h.update(HMAC_DOMAIN);
        h.update(token.as_bytes());
        Secret {
            token: Zeroizing::new(token.to_string()),
            hmac_key: Zeroizing::new(h.finalize().to_vec()),
        }
    }

    /// A fresh 256-bit token from the OS CSPRNG.
    fn generate() -> Result<Self, AuthError> {
        let mut raw = [0u8; TOKEN_BYTES];
        getrandom::getrandom(&mut raw).map_err(|e| AuthError::Rng(e.to_string()))?;
        Ok(Secret::from_token(&hex(&raw)))
    }

    fn token(&self) -> &str {
        &self.token
    }

    fn hmac_key(&self) -> &[u8] {
        &self.hmac_key
    }
}

/// Nonces seen inside the freshness window, so a captured signed request cannot
/// be replayed within it.
struct NonceCache {
    seen: HashMap<String, Instant>,
}

impl NonceCache {
    fn new() -> Self {
        NonceCache {
            seen: HashMap::new(),
        }
    }

    /// Remember `nonce`, refusing one already seen inside `ttl`.
    ///
    /// Called only after the signature has verified, so a forged request cannot
    /// pollute the cache and evict a legitimate caller's entry.
    fn record(&mut self, nonce: &str, ttl: Duration, now: Instant) -> Result<(), AuthError> {
        self.prune(ttl, now);
        if self.seen.contains_key(nonce) {
            return Err(AuthError::ReplayedNonce);
        }
        if self.seen.len() >= NONCE_CACHE_CAP {
            // A full cache is a burst, not something we can tell apart from an
            // attack — evict the oldest and keep accepting, rather than failing
            // closed on a busy caller.
            if let Some(oldest) = self
                .seen
                .iter()
                .min_by_key(|(_, t)| **t)
                .map(|(k, _)| k.clone())
            {
                self.seen.remove(&oldest);
            }
        }
        self.seen.insert(nonce.to_string(), now);
        Ok(())
    }

    fn prune(&mut self, ttl: Duration, now: Instant) {
        self.seen
            .retain(|_, t| now.saturating_duration_since(*t) < ttl);
    }
}

/// The connector's authentication state, shared with the running server so a
/// rotation takes effect without a restart.
pub struct Auth {
    secret: RwLock<Arc<Secret>>,
    nonces: Mutex<NonceCache>,
    backend: Mutex<Backend>,
    ttl: Duration,
    /// Where a rotated token is written when the backend is a file.
    file: PathBuf,
}

impl Auth {
    /// Load the token, generating and persisting one on first run.
    ///
    /// Resolution order is deliberate: an explicit environment override wins,
    /// then the platform credential store, then a 0600 file beside the app's
    /// database. A keyring that is unavailable — a headless Linux box with no
    /// Secret Service on the bus, most often — is an expected outcome rather
    /// than an error, and simply falls through to the file.
    pub fn load_or_init(dir: &Path) -> Result<Arc<Auth>, AuthError> {
        let file = dir.join(TOKEN_FILE);
        let (secret, backend) = resolve(&file)?;
        Ok(Arc::new(Auth {
            secret: RwLock::new(Arc::new(secret)),
            nonces: Mutex::new(NonceCache::new()),
            backend: Mutex::new(backend),
            ttl: ttl_from_env(),
            file,
        }))
    }

    /// The current secret, cloned out so a concurrent rotation cannot swap it
    /// mid-request.
    fn secret(&self) -> Arc<Secret> {
        self.secret.read().unwrap().clone()
    }

    pub fn backend(&self) -> Backend {
        *self.backend.lock().unwrap()
    }

    pub fn ttl_secs(&self) -> u64 {
        self.ttl.as_secs()
    }

    /// A short, non-secret identifier for the current token, so the desktop can
    /// show *which* token is live without ever rendering the token itself.
    pub fn fingerprint(&self) -> String {
        let digest = Sha256::digest(self.secret().token().as_bytes());
        hex(&digest).chars().take(12).collect()
    }

    /// The token itself. Reaching the desktop webview is the only intended use:
    /// the user has to copy it into their MCP host, and the desktop is already
    /// the approval authority. It is never logged and never goes into
    /// `mcp_status`.
    pub fn reveal(&self) -> String {
        self.secret().token().to_string()
    }

    /// Check a request. The bearer token is required; a signature is verified
    /// strictly when present.
    pub fn verify(&self, headers: &HeaderMap, body: &[u8]) -> Result<(), AuthError> {
        let presented = bearer(headers).ok_or(AuthError::MissingCredentials)?;
        let secret = self.secret();
        if !constant_time_eq(presented.as_bytes(), secret.token().as_bytes()) {
            return Err(AuthError::BadToken);
        }
        // Past this point the caller has proven who they are, so a reason
        // returned below tells them something they are entitled to know and
        // tells an unauthenticated prober nothing.
        self.verify_signature(headers, body, &secret)
    }

    fn verify_signature(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        secret: &Secret,
    ) -> Result<(), AuthError> {
        // Opportunistic: no signature, no signature checks.
        let Some(signature) = header(headers, SIGNATURE_HEADER) else {
            return Ok(());
        };
        let timestamp =
            header(headers, TIMESTAMP_HEADER).ok_or(AuthError::MissingSignatureParts)?;
        let nonce = header(headers, NONCE_HEADER).ok_or(AuthError::MissingSignatureParts)?;

        let ttl = self.ttl.as_secs();
        let sent: u64 = timestamp
            .parse()
            .map_err(|_| AuthError::StaleTimestamp(ttl))?;
        if now_secs().abs_diff(sent) > ttl {
            return Err(AuthError::StaleTimestamp(ttl));
        }

        // Integrity first: it is cheap, and it keeps a forged request from
        // costing a cache entry.
        let expected = sign_with(secret.hmac_key(), &timestamp, &nonce, body);
        if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
            return Err(AuthError::BadSignature);
        }

        self.nonces
            .lock()
            .unwrap()
            .record(&nonce, self.ttl, Instant::now())
    }

    /// Sign a response body. Unlike the request direction this is unconditional,
    /// because the server — not the client — is what produces it.
    pub fn sign_response(&self, body: &[u8]) -> String {
        let mut mac =
            HmacSha256::new_from_slice(self.secret().hmac_key()).expect("HMAC takes any key size");
        mac.update(hex(&Sha256::digest(body)).as_bytes());
        hex(&mac.finalize().into_bytes())
    }

    /// Verify a response body against [`Auth::sign_response`]. For first-party
    /// tooling — no MCP client checks a response signature.
    pub fn verify_response(&self, body: &[u8], signature: &str) -> bool {
        constant_time_eq(signature.as_bytes(), self.sign_response(body).as_bytes())
    }

    /// The three headers a signed request needs, freshly nonced. Exposed so
    /// first-party callers (tests, scripts, an edge verifier) can speak the same
    /// scheme.
    pub fn sign_request(&self, body: &[u8]) -> Result<(String, String, String), AuthError> {
        let mut raw = [0u8; 16];
        getrandom::getrandom(&mut raw).map_err(|e| AuthError::Rng(e.to_string()))?;
        let timestamp = now_secs().to_string();
        let nonce = hex(&raw);
        let signature = sign_with(self.secret().hmac_key(), &timestamp, &nonce, body);
        Ok((timestamp, nonce, signature))
    }

    /// Replace the token, persisting it before it goes live.
    ///
    /// Persist-first is the whole point: a rotation that took effect in memory
    /// but never reached storage would work until the next launch and then
    /// break, with nothing on screen to explain why.
    pub fn rotate(&self) -> Result<(), AuthError> {
        let current = self.backend();
        if current == Backend::Env {
            return Err(AuthError::EnvOverride);
        }
        let secret = Secret::generate()?;
        // Write back to the store already in use, rather than retrying the
        // keyring and quietly moving a file-backed token into it.
        let used = persist(&self.file, &secret, current == Backend::Keyring)?;
        *self.secret.write().unwrap() = Arc::new(secret);
        *self.backend.lock().unwrap() = used;
        Ok(())
    }

    /// An ephemeral in-memory instance for tests: no keyring, no disk.
    #[cfg(test)]
    pub fn for_tests() -> Arc<Auth> {
        Arc::new(Auth {
            secret: RwLock::new(Arc::new(Secret::from_token("test-token"))),
            nonces: Mutex::new(NonceCache::new()),
            backend: Mutex::new(Backend::File),
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
            file: PathBuf::new(),
        })
    }

    /// A file-backed instance for tests, built without consulting the
    /// environment or the credential store.
    ///
    /// Hermetic on purpose: a developer with `LEXSUS_MCP_AUTH_TOKEN` exported,
    /// or a real keyring entry from running the app, must not change what a
    /// test observes.
    #[cfg(test)]
    pub fn for_tests_in(dir: &Path) -> Arc<Auth> {
        let file = dir.join(TOKEN_FILE);
        let secret = Secret::generate().expect("test token");
        file_store(&file, secret.token()).expect("test token file");
        Arc::new(Auth {
            secret: RwLock::new(Arc::new(secret)),
            nonces: Mutex::new(NonceCache::new()),
            backend: Mutex::new(Backend::File),
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
            file,
        })
    }
}

// --- resolution --------------------------------------------------------------

fn resolve(file: &Path) -> Result<(Secret, Backend), AuthError> {
    if let Some(token) = env_token() {
        return Ok((Secret::from_token(&token), Backend::Env));
    }
    if let Some(token) = keyring_load() {
        return Ok((Secret::from_token(&token), Backend::Keyring));
    }
    if let Some(token) = file_load(file) {
        return Ok((Secret::from_token(&token), Backend::File));
    }
    // First run: generate, then try to persist somewhere it survives a restart.
    // A token that cannot be generated is fatal for the connector — the caller
    // leaves the endpoint unbound rather than serving it unauthenticated.
    let secret = Secret::generate()?;
    let backend = persist(file, &secret, true)?;
    Ok((secret, backend))
}

/// Persist `secret`, reporting which backend took it.
///
/// `prefer_keyring` is false when the credential store is unavailable (a
/// headless Linux box has no Secret Service on the bus) or when the token is
/// already living in a file — in which case going back to the same file is what
/// keeps a rotation from silently migrating stores.
fn persist(file: &Path, secret: &Secret, prefer_keyring: bool) -> Result<Backend, AuthError> {
    if prefer_keyring && keyring_store(secret.token()) {
        return Ok(Backend::Keyring);
    }
    file_store(file, secret.token()).map_err(|e| AuthError::Persist(e.to_string()))?;
    Ok(Backend::File)
}

fn ttl_from_env() -> Duration {
    let secs = std::env::var("LEXSUS_MCP_SIGNATURE_TTL_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(DEFAULT_TTL_SECS);
    Duration::from_secs(secs)
}

fn env_token() -> Option<String> {
    let raw = std::env::var("LEXSUS_MCP_AUTH_TOKEN").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn keyring_load() -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()?;
    match entry.get_password() {
        Ok(token) if !token.trim().is_empty() => Some(token.trim().to_string()),
        _ => None,
    }
}

fn keyring_store(token: &str) -> bool {
    match keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
        Ok(entry) => entry.set_password(token).is_ok(),
        Err(_) => false,
    }
}

fn file_load(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Write the token 0600 via a temporary file, so it is never briefly readable
/// under its final name.
fn file_store(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}

// --- primitives --------------------------------------------------------------

/// The canonical string a signature covers.
///
/// The body enters as its own digest, which makes the parts unambiguous — a
/// body that happens to contain a newline cannot be made to look like a
/// different (timestamp, nonce) pair.
fn signing_string(timestamp: &str, nonce: &str, body: &[u8]) -> String {
    format!("{timestamp}\n{nonce}\n{}", hex(&Sha256::digest(body)))
}

fn sign_with(key: &[u8], timestamp: &str, nonce: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key size");
    mac.update(signing_string(timestamp, nonce, body).as_bytes());
    hex(&mac.finalize().into_bytes())
}

/// Constant-time equality over two secrets of any length.
///
/// Each side is hashed first so the comparison is always between two 32-byte
/// values: comparing raw would both leak length through timing and give
/// `ct_eq` mismatched lengths to reject outright.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    Sha256::digest(a).ct_eq(&Sha256::digest(b)).into()
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    /// Headers carrying a valid bearer, plus the signature set the caller sent.
    fn signed_headers(auth: &Auth, body: &[u8], token: &str) -> HeaderMap {
        let (ts, nonce, sig) = auth.sign_request(body).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers.insert(TIMESTAMP_HEADER, HeaderValue::from_str(&ts).unwrap());
        headers.insert(NONCE_HEADER, HeaderValue::from_str(&nonce).unwrap());
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        headers
    }

    fn bearer_only(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }

    #[test]
    fn bearer_alone_authorises() {
        let auth = Auth::for_tests();
        let body = br#"{"jsonrpc":"2.0","method":"tools/list"}"#;
        assert_eq!(auth.verify(&bearer_only("test-token"), body), Ok(()));
    }

    #[test]
    fn missing_and_wrong_tokens_are_refused() {
        let auth = Auth::for_tests();
        let body = b"{}";
        assert_eq!(
            auth.verify(&HeaderMap::new(), body),
            Err(AuthError::MissingCredentials)
        );
        assert_eq!(
            auth.verify(&bearer_only("wrong"), body),
            Err(AuthError::BadToken)
        );
        // The scheme has to be bearer; a bare token is not one.
        let mut raw = HeaderMap::new();
        raw.insert(AUTHORIZATION, HeaderValue::from_static("test-token"));
        assert_eq!(auth.verify(&raw, body), Err(AuthError::MissingCredentials));
    }

    #[test]
    fn a_presented_signature_round_trips() {
        let auth = Auth::for_tests();
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#;
        let headers = signed_headers(&auth, body, "test-token");
        assert_eq!(auth.verify(&headers, body), Ok(()));
    }

    /// The signature covers the body: the same headers over different bytes
    /// must not verify.
    #[test]
    fn a_tampered_body_fails_its_signature() {
        let auth = Auth::for_tests();
        let body = br#"{"path":"notes.txt"}"#;
        let headers = signed_headers(&auth, body, "test-token");
        let tampered = br#"{"path":"../../etc/shadow"}"#;
        assert_eq!(
            auth.verify(&headers, tampered),
            Err(AuthError::BadSignature)
        );
    }

    #[test]
    fn a_stale_or_malformed_timestamp_is_refused() {
        let auth = Auth::for_tests();
        let body = b"{}";
        let (_, nonce, sig) = auth.sign_request(body).unwrap();

        for stamp in [
            "0".to_string(),
            (now_secs() + DEFAULT_TTL_SECS + 60).to_string(),
            "not-a-number".to_string(),
        ] {
            let mut headers = bearer_only("test-token");
            headers.insert(TIMESTAMP_HEADER, HeaderValue::from_str(&stamp).unwrap());
            headers.insert(NONCE_HEADER, HeaderValue::from_str(&nonce).unwrap());
            headers.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
            assert!(
                matches!(
                    auth.verify(&headers, body),
                    Err(AuthError::StaleTimestamp(_))
                ),
                "timestamp {stamp} should not be accepted"
            );
        }
    }

    /// Replaying a captured request verbatim is refused the second time.
    #[test]
    fn a_replayed_nonce_is_refused() {
        let auth = Auth::for_tests();
        let body = b"{}";
        let headers = signed_headers(&auth, body, "test-token");
        assert_eq!(auth.verify(&headers, body), Ok(()));
        assert_eq!(auth.verify(&headers, body), Err(AuthError::ReplayedNonce));
    }

    /// A signature without its companion headers is a malformed request, not a
    /// request with no signature.
    #[test]
    fn a_signature_needs_its_timestamp_and_nonce() {
        let auth = Auth::for_tests();
        let body = b"{}";
        let (_, _, sig) = auth.sign_request(body).unwrap();
        let mut headers = bearer_only("test-token");
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_str(&sig).unwrap());
        assert_eq!(
            auth.verify(&headers, body),
            Err(AuthError::MissingSignatureParts)
        );
    }

    #[test]
    fn responses_round_trip_and_detect_edits() {
        let auth = Auth::for_tests();
        let body = br#"{"result":{"tools":[]}}"#;
        let sig = auth.sign_response(body);
        assert!(auth.verify_response(body, &sig));
        assert!(!auth.verify_response(br#"{"result":{"tools":["x"]}}"#, &sig));
    }

    /// The signing key is not the bearer token, so leaking one does not hand
    /// over the other.
    #[test]
    fn the_signing_key_is_derived_not_the_token() {
        let secret = Secret::from_token("abc");
        assert_ne!(secret.hmac_key(), b"abc");
        assert_eq!(secret.hmac_key().len(), 32);
        // Deterministic, so a restart re-derives the same key.
        assert_eq!(Secret::from_token("abc").hmac_key(), secret.hmac_key());
        assert_ne!(Secret::from_token("abd").hmac_key(), secret.hmac_key());
    }

    #[test]
    fn the_fingerprint_identifies_without_disclosing() {
        let auth = Auth::for_tests();
        let fp = auth.fingerprint();
        assert_eq!(fp.len(), 12);
        assert!(!auth.reveal().contains(&fp));
        assert_ne!(fp, auth.reveal());
        // Stable across calls, so the UI can show it as an identity.
        assert_eq!(fp, auth.fingerprint());
    }

    /// Rotation replaces the live token immediately, and the old one stops
    /// working without a restart.
    #[test]
    fn rotation_invalidates_the_previous_token() {
        let dir = std::env::temp_dir().join(format!("lexsus-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Hermetic: neither the environment nor the developer's real keyring
        // takes part. A test must never write to the credential store.
        let auth = Auth::for_tests_in(&dir);
        let before = auth.reveal();
        let before_fingerprint = auth.fingerprint();

        auth.rotate().unwrap();
        let after = auth.reveal();

        assert_ne!(before, after);
        assert_ne!(auth.fingerprint(), before_fingerprint);
        assert_eq!(
            auth.verify(&bearer_only(&before), b"{}"),
            Err(AuthError::BadToken)
        );
        assert_eq!(auth.verify(&bearer_only(&after), b"{}"), Ok(()));
        // Persisted, so the new token survives a restart.
        assert_eq!(
            file_load(&dir.join(TOKEN_FILE)).as_deref(),
            Some(after.as_str())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A generated token is full-entropy hex, not a short or patterned value.
    #[test]
    fn generated_tokens_are_wide_and_distinct() {
        let a = Secret::generate().unwrap();
        let b = Secret::generate().unwrap();
        assert_eq!(a.token().len(), TOKEN_BYTES * 2);
        assert!(a.token().chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a.token(), b.token());
    }

    /// The file fallback is what a headless Linux box actually uses, so its
    /// permissions are load-bearing.
    #[test]
    fn the_file_fallback_is_owner_only() {
        let dir = std::env::temp_dir().join(format!("lexsus-auth-perm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(TOKEN_FILE);
        file_store(&path, "secret-value").unwrap();

        assert_eq!(file_load(&path).as_deref(), Some("secret-value"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "token file is not owner-only");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cache_prunes_and_refuses_repeats_within_the_window() {
        let mut cache = NonceCache::new();
        let now = Instant::now();
        let ttl = Duration::from_secs(60);

        assert_eq!(cache.record("a", ttl, now), Ok(()));
        assert_eq!(cache.record("a", ttl, now), Err(AuthError::ReplayedNonce));
        assert_eq!(cache.record("b", ttl, now), Ok(()));

        // Past the window the same nonce is fresh again, and the map has been
        // swept rather than accumulating forever.
        let later = now + Duration::from_secs(61);
        assert_eq!(cache.record("a", ttl, later), Ok(()));
        assert_eq!(cache.seen.len(), 1);
    }
}
