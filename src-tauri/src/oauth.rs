//! OAuth 2.1 authorization-code + PKCE for the MCP connector.
//!
//! A hosted connector (Claude.ai) cannot send a static `Authorization` header,
//! so the static bearer token is useless to it. What it *can* do is the
//! standard MCP OAuth dance: read the authorization-server metadata off the
//! 401's `WWW-Authenticate`, run an authorization-code + PKCE flow against
//! `/authorize` and `/token`, then present the issued access token as a bearer.
//! This module is that authorization server.
//!
//! Tokens and codes are opaque random values held in memory. They do not need
//! to survive a restart: a reconnected client simply re-authenticates. The
//! static token from [`crate::auth::Auth`] keeps working alongside them.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ACCESS_TOKEN_TTL: Duration = Duration::from_secs(3600);
const CODE_TTL: Duration = Duration::from_secs(300);
const TOKEN_BYTES: usize = 32;

/// A not-yet-exchanged authorization code, plus everything the exchange must
/// prove it was issued for.
#[derive(Clone)]
struct Pending {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    expires: Instant,
}

/// Shared OAuth state. Both this and [`crate::auth::Auth`] are held in memory
/// and cloned into the axum router, so a connector restart simply starts over.
pub struct OAuthServer {
    codes: Mutex<HashMap<String, Pending>>,
    tokens: Mutex<HashMap<String, Instant>>,
}

impl OAuthServer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            codes: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
        })
    }

    /// Whether `token` is a live access token we issued.
    pub fn check_access_token(&self, token: &str) -> bool {
        let mut tokens = self.tokens.lock().unwrap();
        let now = Instant::now();
        tokens.retain(|_, exp| *exp > now);
        tokens.contains_key(token)
    }

    fn issue_code(&self, pending: Pending) -> String {
        let code = random_hex();
        self.codes.lock().unwrap().insert(code.clone(), pending);
        code
    }

    fn issue_token(&self) -> String {
        let token = random_hex();
        self.tokens
            .lock()
            .unwrap()
            .insert(token.clone(), Instant::now() + ACCESS_TOKEN_TTL);
        token
    }

    /// Exchange a one-time code for an access token, verifying PKCE.
    fn exchange(
        &self,
        code: &str,
        verifier: &str,
        client_id: &str,
        redirect_uri: &str,
    ) -> Option<String> {
        let pending = {
            let mut codes = self.codes.lock().unwrap();
            codes.remove(code)?
        };
        if pending.expires < Instant::now()
            || pending.client_id != client_id
            || pending.redirect_uri != redirect_uri
            || !pkce_verify(&pending.code_challenge, verifier)
        {
            return None;
        }
        Some(self.issue_token())
    }
}

/// The public OAuth routes, mounted *outside* the connector's auth middleware.
pub fn router(oauth: Arc<OAuthServer>) -> axum::Router {
    axum::Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(resource_metadata),
        )
        .route("/authorize", get(authorize))
        .route("/authorize/approve", get(approve))
        .route("/token", post(token))
        .with_state(oauth)
}

// --- handlers ----------------------------------------------------------------

/// RFC 8414 authorization-server metadata.
async fn oauth_metadata(headers: HeaderMap) -> Response {
    let base = base_url(&headers);
    json_response(
        StatusCode::OK,
        serde_json::json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "scopes_supported": ["mcp"],
        }),
    )
}

/// RFC 9728 protected-resource metadata.
async fn resource_metadata(headers: HeaderMap) -> Response {
    let base = base_url(&headers);
    json_response(
        StatusCode::OK,
        serde_json::json!({
            "resource": format!("{base}/mcp"),
            "authorization_servers": [format!("{base}/.well-known/oauth-authorization-server")],
            "scopes_supported": ["mcp"],
            "bearer_methods_supported": ["header"],
        }),
    )
}

/// The consent step. Renders a tiny page with an Allow button; approving
/// redirects the browser to `/authorize/approve` with the original parameters
/// intact.
async fn authorize(headers: HeaderMap, Query(params): Query<HashMap<String, String>>) -> Response {
    let response_type = params
        .get("response_type")
        .map(String::as_str)
        .unwrap_or("");
    if response_type != "code" {
        return json_response(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "invalid_request", "error_description": "response_type must be 'code'" }),
        );
    }
    let approve = format!(
        "{}/authorize/approve?{}",
        base_url(&headers),
        encode_query(&params)
    );
    Html(CONSENT_PAGE.replace("{approve}", &approve)).into_response()
}

/// Approve and redirect back with a freshly issued code.
async fn approve(
    State(oauth): State<Arc<OAuthServer>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let redirect_uri = params.get("redirect_uri").cloned().unwrap_or_default();
    let state = params.get("state").cloned();
    let client_id = params.get("client_id").cloned().unwrap_or_default();
    let code_challenge = params.get("code_challenge").cloned().unwrap_or_default();
    let method = params
        .get("code_challenge_method")
        .cloned()
        .unwrap_or_default();

    if method != "S256" {
        return redirect_error(&redirect_uri, state.as_deref(), "invalid_request");
    }

    let code = oauth.issue_code(Pending {
        client_id,
        redirect_uri: redirect_uri.clone(),
        code_challenge,
        expires: Instant::now() + CODE_TTL,
    });

    let mut target = redirect_uri;
    target.push(if target.contains('?') { '&' } else { '?' });
    target.push_str("code=");
    target.push_str(&code);
    if let Some(s) = &state {
        target.push_str("&state=");
        target.push_str(&url_encode(s));
    }
    Redirect::to(&target).into_response()
}

/// Token endpoint: authorization-code + PKCE exchange.
async fn token(State(oauth): State<Arc<OAuthServer>>, body: String) -> Response {
    let form = parse_form(body.as_bytes());
    let grant = form.get("grant_type").map(String::as_str).unwrap_or("");
    if grant != "authorization_code" {
        return token_error(
            "unsupported_grant_type",
            "only authorization_code is supported",
        );
    }

    let code = form.get("code").cloned().unwrap_or_default();
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let client_id = form.get("client_id").cloned().unwrap_or_default();
    let redirect_uri = form.get("redirect_uri").cloned().unwrap_or_default();

    match oauth.exchange(&code, &verifier, &client_id, &redirect_uri) {
        Some(access_token) => {
            let mut res = json_response(
                StatusCode::OK,
                serde_json::json!({
                    "access_token": access_token,
                    "token_type": "Bearer",
                    "expires_in": ACCESS_TOKEN_TTL.as_secs(),
                    "scope": "mcp",
                }),
            );
            res.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
            res
        }
        None => token_error(
            "invalid_grant",
            "code, verifier, or redirect_uri did not match",
        ),
    }
}

// --- helpers ----------------------------------------------------------------

/// The public base URL this request arrived through, derived from `Host`. A
/// tunnel host is `https`; loopback is `http`. Shared with the MCP layer so
/// the 401's `WWW-Authenticate` can point at the same discovery URLs.
pub fn base_url(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1");
    let scheme = if is_loopback(host) { "http" } else { "https" };
    format!("{scheme}://{host}")
}

fn is_loopback(host: &str) -> bool {
    let host = host.split(':').next().unwrap_or(host);
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "127.0.0.1"
        || host.starts_with("127.")
        || host == "::1"
        || host == "[::1]"
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    let mut res = Response::new(Body::from(value.to_string()));
    *res.status_mut() = status;
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    res
}

fn token_error(code: &str, description: &str) -> Response {
    json_response(
        StatusCode::BAD_REQUEST,
        serde_json::json!({ "error": code, "error_description": description }),
    )
}

fn redirect_error(redirect_uri: &str, state: Option<&str>, error: &str) -> Response {
    let mut target = redirect_uri.to_string();
    target.push(if target.contains('?') { '&' } else { '?' });
    target.push_str("error=");
    target.push_str(error);
    if let Some(s) = state {
        target.push_str("&state=");
        target.push_str(&url_encode(s));
    }
    Redirect::to(&target).into_response()
}

/// SHA256 + base64url, as PKCE S256 requires.
fn pkce_verify(challenge: &str, verifier: &str) -> bool {
    use sha2::{Digest, Sha256};
    challenge == base64url(&Sha256::digest(verifier.as_bytes()))
}

fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    // base64url drops padding
    out.trim_end_matches('=').to_string()
}

fn parse_form(body: &[u8]) -> HashMap<String, String> {
    let text = String::from_utf8_lossy(body);
    let mut out = HashMap::new();
    for pair in text.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(percent_decode(k), percent_decode(v));
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hi) = hex_val(bytes[i + 1]) {
                if let Ok(lo) = hex_val(bytes[i + 2]) {
                    out.push((hi << 4) | lo);
                    i += 3;
                    continue;
                }
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(()),
    }
}

fn encode_query(params: &HashMap<String, String>) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn random_hex() -> String {
    let mut raw = [0u8; TOKEN_BYTES];
    getrandom::getrandom(&mut raw).expect("OS CSPRNG");
    use std::fmt::Write;
    let mut out = String::with_capacity(TOKEN_BYTES * 2);
    for b in raw {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The consent page. Kept as a plain string so it needs no templating.
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn pkce_s256_verifies() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        assert!(pkce_verify(&challenge, verifier));
        assert!(!pkce_verify(&challenge, "wrong-verifier"));
    }

    #[test]
    fn a_code_exchanges_once_with_the_right_verifier() {
        let oauth = OAuthServer::new();
        let challenge = base64url(&Sha256::digest(b"verifier"));

        // A wrong verifier does not mint a token.
        let bad = oauth.issue_code(Pending {
            client_id: "claude".into(),
            redirect_uri: "https://callback".into(),
            code_challenge: challenge.clone(),
            expires: Instant::now() + CODE_TTL,
        });
        assert!(oauth
            .exchange(&bad, "nope", "claude", "https://callback")
            .is_none());

        // The right verifier does, once.
        let good = oauth.issue_code(Pending {
            client_id: "claude".into(),
            redirect_uri: "https://callback".into(),
            code_challenge: challenge,
            expires: Instant::now() + CODE_TTL,
        });
        let token = oauth
            .exchange(&good, "verifier", "claude", "https://callback")
            .expect("valid exchange");
        assert!(oauth.check_access_token(&token));
        // Codes are single-use.
        assert!(oauth
            .exchange(&good, "verifier", "claude", "https://callback")
            .is_none());
    }
}

const CONSENT_PAGE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Connect to Lexsus</title>
<style>
  body { font-family: system-ui, sans-serif; background:#fafafa; color:#1a1a1a;
         display:grid; place-items:center; min-height:100vh; margin:0; }
  .card { background:#fff; border:1px solid #e5e5e5; border-radius:12px;
          padding:32px; max-width:420px; box-shadow:0 10px 40px rgba(0,0,0,.06); }
  h1 { font-size:20px; margin:0 0 8px; }
  p { color:#555; font-size:14px; line-height:1.6; margin:0 0 20px; }
  .row { display:flex; gap:10px; }
  .allow { background:#0d9488; color:#fff; border:0; border-radius:8px;
           padding:10px 18px; font-size:14px; cursor:pointer; text-decoration:none; }
  .deny { background:#fff; color:#555; border:1px solid #d4d4d4; border-radius:8px;
          padding:10px 18px; font-size:14px; cursor:pointer; }
</style>
</head>
<body>
  <div class="card">
    <h1>Connect to Lexsus</h1>
    <p>A hosted AI is asking to use your local Lexsus MCP server. Approving
       issues it a short-lived access token. Stop the tunnel or the connector
       to revoke access.</p>
    <div class="row">
      <a class="allow" href="{approve}">Allow</a>
    </div>
  </div>
</body>
</html>"#;
