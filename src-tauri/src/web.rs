//! `web_fetch`: the one tool that reaches off the machine.
//!
//! Every other tool in the engine answers a question about the workspace, and
//! the worst it can do is damage the workspace. This one sends a request to a
//! host somebody else chose and brings back text somebody else wrote — so it
//! is the only tool whose *policy* is more interesting than its mechanics, and
//! most of this module is that policy.
//!
//! **The address guard is the point.** A URL is not a place, it is a name that
//! resolves to one, and the whole class of server-side-request-forgery bugs
//! comes from trusting the name. `http://metadata.internal/` and
//! `http://169.254.169.254/` are the same place; a name that resolves *now* to
//! a public address may resolve to a private one by the time the connection is
//! made. So the name is resolved here, the resolved address is checked, and
//! the connection is **pinned** to the address that was checked — the resolver
//! does not get a second, unexamined say. Redirects are followed here too,
//! one hop at a time, through the same check, rather than by the HTTP client,
//! so a 302 cannot walk the request somewhere the first URL was not allowed to
//! go.
//!
//! **Loopback is deliberately reachable.** The block-list is about *not*
//! reaching the local network and the cloud's internal services; `127.0.0.1`
//! is neither — it is the machine the user is already sitting at, and looking
//! at a dev server the user just started is an ordinary thing for a coding
//! agent to do. It is also what makes every fetch path in this module testable
//! against a real socket without the suite touching the internet. The residual
//! risk — a fetched page that injects a prompt telling the model to read a
//! local admin panel — is real and is why this tool's approval class is
//! `Always`: the user sees every host before a byte leaves.
//!
//! **Caps stop work rather than truncating output.** The body is read chunk by
//! chunk and the connection is dropped the moment the cap is reached, so a
//! hundred-megabyte response costs what the cap says and not what the server
//! wanted to send.
//!
//! **HTML is reduced, not parsed.** [`html_to_text`] is a scanner: it drops
//! `<script>`/`<style>` subtrees, drops every tag, decodes the entities that
//! actually matter and collapses whitespace. It has no notion of where in a
//! document a tag may legally appear, so its output is text a reader can
//! follow, not a faithful rendering — and the page's raw byte count is
//! reported alongside it so nothing about the reduction is hidden.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use regex::Regex;
use reqwest::Url;

/// Most bytes one fetch will read off the wire. A hard ceiling, not a
/// default: `max_bytes` asks for less, never more.
pub const MAX_BYTES: usize = 512 * 1024;

/// What a fetch reads when the caller says nothing.
pub const DEFAULT_BYTES: usize = 128 * 1024;

/// Redirects followed before giving up. Bounded because each hop is a fresh
/// request the caller did not ask for.
const MAX_REDIRECTS: usize = 5;

/// Per-request timeout *and* total budget: the client is rebuilt per hop, so
/// a chain of slow redirects cannot multiply this by the hop count by much.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Longest URL accepted. Long enough for any real one, short enough that a
/// URL is never a body in disguise.
const MAX_URL_CHARS: usize = 2048;

/// Who we say we are. Some servers refuse a request with no user agent, and a
/// tool that pretends to be a browser would be lying to the server that will
/// have to serve it.
const USER_AGENT: &str = concat!("lexsus-mcp/", env!("CARGO_PKG_VERSION"));

// --- the address policy ------------------------------------------------------

/// Why an address may not be connected to. `None` means it may.
///
/// Read this as the answer to "can a URL make this tool reach something the
/// user did not mean to expose?" — every range named here is a thing that
/// lives *inside* a network boundary and has no business being named by a
/// public URL: the cloud metadata service (link-local), the local network, a
/// carrier's internal range, and the addresses that are not destinations at
/// all. Loopback is absent on purpose; see the module docs.
pub fn blocked_range(ip: IpAddr) -> Option<&'static str> {
    Some(match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            match (a, b) {
                // The cloud metadata service answers here. It is the single
                // most valuable address to reach from inside a VM and it
                // hands out credentials to anyone who asks.
                (169, 254) => "link-local — cloud metadata is served in this range",
                (10, _) => "private",
                (172, 16..=31) => "private",
                (192, 168) => "private",
                (100, 64..=127) => "carrier-grade NAT",
                (0, _) => "unspecified",
                (a, _) if a >= 224 => "multicast or reserved",
                _ => return None,
            }
        }
        IpAddr::V6(v6) => {
            // An IPv4 address written as IPv6 is the same address, so it gets
            // the IPv4 verdict rather than a second, weaker one.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return blocked_range(IpAddr::V4(v4));
            }
            if v6.is_loopback() {
                return None;
            }
            let head = v6.segments()[0];
            match head {
                _ if head & 0xffc0 == 0xfe80 => {
                    "link-local — cloud metadata is served in this range"
                }
                _ if head & 0xfe00 == 0xfc00 => "unique-local (private)",
                0 => "unspecified",
                _ if head & 0xff00 == 0xff00 => "multicast",
                // `::ffff:0:0/96` is the IPv4-mapped range and was handled
                // above; `64:ff9b::/96` is NAT64 and is not worth a rule of
                // its own here, since it resolves to a real destination.
                _ => return None,
            }
        }
    })
}

// --- the planner (pure) ------------------------------------------------------

/// A URL that passed the policy, with the cap it will be read under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub url: Url,
    pub max_bytes: usize,
    /// When true the body is passed through as text even if the content type
    /// is HTML. `web_fetch` wants readable prose; `web_search` needs the
    /// markup, because the links it is looking for are attributes and a
    /// reduction would delete exactly the part it needs.
    pub raw: bool,
}

/// Why a fetch was refused before any connection was made.
///
/// Every variant is a *refusal*, not a failure: nothing was sent, nothing was
/// received, and the caller can act on all of them. They all surface as
/// `NETWORK_BLOCKED`, because that is the one thing the caller needs to know —
/// this tool will not fetch that — while the message says which and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    InvalidUrl(String),
    UnsupportedScheme(String),
    MissingHost,
    /// A URL carrying credentials would have them sent to the host on the
    /// next hop as well, and a URL is the least private place to put a secret.
    Credentials,
    /// Every address the host resolved to is one the address policy refuses.
    BlockedAddress {
        host: String,
        addr: IpAddr,
        range: &'static str,
    },
    Unresolvable {
        host: String,
        error: String,
    },
    TooManyRedirects {
        limit: usize,
    },
    /// The request itself failed — DNS, TLS, connection, timeout.
    Request(String),
}

impl Refusal {
    /// One sentence a caller can act on. Names the reason, and where the
    /// reason is a range, names the range.
    pub fn message(&self) -> String {
        match self {
            Refusal::InvalidUrl(u) => format!("'{u}' is not a URL this tool can fetch"),
            Refusal::UnsupportedScheme(s) => format!(
                "scheme '{s}' is not fetchable — only http and https leave the machine"
            ),
            Refusal::MissingHost => "the URL has no host to fetch from".to_string(),
            Refusal::Credentials => {
                "the URL carries credentials; fetch it without them and authenticate another way"
                    .to_string()
            }
            Refusal::BlockedAddress { host, addr, range } => format!(
                "'{host}' resolves to {addr}, which is {range} — refused before connecting"
            ),
            Refusal::Unresolvable { host, error } => {
                format!("could not resolve '{host}': {error}")
            }
            Refusal::TooManyRedirects { limit } => {
                format!("more than {limit} redirects; the last one was not followed")
            }
            Refusal::Request(e) => format!("the request failed: {e}"),
        }
    }

    /// Whether the refusal is *about the address* rather than the URL's shape.
    /// Only these can be re-decided by a later hop.
    pub fn is_address(&self) -> bool {
        matches!(self, Refusal::BlockedAddress { .. })
    }
}

/// Validate a URL and decide the read cap. Pure: no name is resolved here, so
/// every rule below is a statement about the URL *text*, which is what makes
/// it testable over a table.
pub fn plan(url: &str, max_bytes: Option<u64>) -> Result<Plan, Refusal> {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_URL_CHARS {
        return Err(Refusal::InvalidUrl(url.to_string()));
    }
    let parsed = Url::parse(trimmed).map_err(|_| Refusal::InvalidUrl(trimmed.to_string()))?;
    check_shape(&parsed)?;
    Ok(Plan {
        max_bytes: cap(max_bytes),
        url: parsed,
        raw: false,
    })
}

/// Plan a URL for a caller that needs the raw body, not readable text.
pub fn plan_raw(url: &str, max_bytes: Option<u64>) -> Result<Plan, Refusal> {
    let mut plan = plan(url, max_bytes)?;
    plan.raw = true;
    Ok(plan)
}

/// The rules a URL must satisfy at *every* hop — the first one and each
/// redirect — so a hop cannot relax what the previous one established.
fn check_shape(url: &Url) -> Result<(), Refusal> {
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(Refusal::UnsupportedScheme(other.to_string())),
    }
    if url.host_str().is_none() {
        return Err(Refusal::MissingHost);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Refusal::Credentials);
    }
    Ok(())
}

/// The read cap, clamped. Absent asks for [`DEFAULT_BYTES`]; anything larger
/// than [`MAX_BYTES`] is the ceiling, and zero is not a way to read nothing —
/// it is a caller who did not think about it, so it gets the default.
fn cap(max_bytes: Option<u64>) -> usize {
    match max_bytes {
        None | Some(0) => DEFAULT_BYTES,
        Some(n) => n.min(MAX_BYTES as u64) as usize,
    }
}

/// The host and port to connect to, and the address policy's verdict on
/// *each* address the name resolved to.
///
/// Refuses only when **every** address is blocked, and returns the first that
/// is not: the request is then pinned to that address, so a host that resolves
/// to both a public address and a link-local one is fetched from the public
/// one rather than refused outright. Refusing the whole name would be the
/// stricter rule and the wrong one — it would make a legitimate multi-homed
/// host unfetchable while the connection still could have been safe.
fn resolve_and_check(url: &Url) -> Result<SocketAddr, Refusal> {
    let host = url.host_str().unwrap_or_default().to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let resolved: Vec<SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| Refusal::Unresolvable {
            host: host.clone(),
            error: e.to_string(),
        })?
        .collect();

    let mut last_blocked = None;
    for addr in resolved {
        match blocked_range(addr.ip()) {
            None => return Ok(addr),
            Some(range) => {
                last_blocked = Some(Refusal::BlockedAddress {
                    host: host.clone(),
                    addr: addr.ip(),
                    range,
                })
            }
        }
    }
    Err(last_blocked.unwrap_or(Refusal::Unresolvable {
        host,
        error: "the name resolved to no addresses".to_string(),
    }))
}

// --- the runner (effectful) --------------------------------------------------

/// What one fetch brought back.
#[derive(Debug, Clone)]
pub struct Page {
    /// Where the bytes actually came from — the last URL in the redirect
    /// chain, which is usually but not always the one that was asked for.
    pub url: String,
    pub status: u16,
    pub status_text: String,
    pub content_type: String,
    /// The body as text, HTML reduced if it was HTML.
    pub body: String,
    /// Body bytes read off the wire, before any reduction.
    pub bytes: usize,
    pub truncated: bool,
    /// The URLs that were redirected through, in order.
    pub redirects: Vec<String>,
    /// Whether the body was HTML and so was reduced rather than passed
    /// through.
    pub reduced_html: bool,
}

impl Page {
    /// Whether the server said the request was fine. A 404 is a *successful
    /// fetch* of an error page, and the page is usually the useful part, so
    /// this is reported rather than turned into a tool failure.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Fetch a planned URL. Blocking; runs its own single-threaded runtime so it
/// can be called from a Tauri command thread or a blocking-pool thread
/// without caring which runtime, if any, is already there.
pub fn fetch(plan: &Plan) -> Result<Page, Refusal> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Refusal::Request(e.to_string()))?;
    runtime.block_on(fetch_hops(plan))
}

/// Follow the chain one hop at a time, checking and pinning each one.
async fn fetch_hops(plan: &Plan) -> Result<Page, Refusal> {
    let mut url = plan.url.clone();
    let mut redirects: Vec<String> = Vec::new();

    for _ in 0..=MAX_REDIRECTS {
        // Re-checked at every hop, not just the first: a redirect is a new
        // URL and earns a new verdict.
        let addr = resolve_and_check(&url)?;
        let host = url.host_str().unwrap_or_default().to_string();

        // Two decisions make the check above mean something. `redirect(none)`
        // takes redirect following away from the client, so the next hop goes
        // through this loop — and this check — instead of around it. `resolve`
        // pins the name to the address that was checked, closing the window
        // between checking and connecting in which a name can be re-pointed.
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&host, addr)
            .build()
            .map_err(|e| Refusal::Request(e.to_string()))?;

        let response = client
            .get(url.clone())
            .send()
            .await
            .map_err(|e| Refusal::Request(e.to_string()))?;
        let status = response.status();

        if status.is_redirection() {
            let Some(location) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
            else {
                // A redirect that does not say where. There is nothing to
                // follow, and the status is the answer.
                return read_body(response, url, Vec::new(), plan).await;
            };
            // Relative locations are resolved against the URL that sent them,
            // which is what makes `Location: /next` work at all.
            let next = url.join(&location).map_err(|e| {
                Refusal::Request(format!("redirect to '{location}' is not a URL: {e}"))
            })?;
            check_shape(&next)?;
            redirects.push(url.to_string());
            url = next;
            continue;
        }

        return read_body(response, url, redirects, plan).await;
    }

    Err(Refusal::TooManyRedirects {
        limit: MAX_REDIRECTS,
    })
}

/// Read the body up to the cap, then stop.
///
/// The `break` on the cap is the whole point of the function: dropping the
/// response here closes the connection, so a server that would have streamed
/// gigabytes is told, by the socket going away, that nothing more is wanted.
/// Reading to the end and then cutting the buffer would pay the full cost of
/// the response for the same answer.
async fn read_body(
    mut response: reqwest::Response,
    url: Url,
    redirects: Vec<String>,
    plan: &Plan,
) -> Result<Page, Refusal> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();

    let mut raw: Vec<u8> = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Refusal::Request(e.to_string()))?
    {
        let room = plan.max_bytes.saturating_sub(raw.len());
        if chunk.len() > room {
            raw.extend_from_slice(&chunk[..room]);
            truncated = true;
            break;
        }
        raw.extend_from_slice(&chunk);
    }

    // Lossy, and said rather than guessed: a page that is not UTF-8 gets
    // replacement characters instead of a mojibake decoding that would look
    // like content. The content type is reported so the caller can see why.
    let text = String::from_utf8_lossy(&raw).into_owned();
    let reduced_html = is_html(&content_type) && !plan.raw;
    let body = if reduced_html {
        html_to_text(&text)
    } else {
        text
    };

    Ok(Page {
        url: url.to_string(),
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_string(),
        content_type,
        body,
        bytes: raw.len(),
        truncated,
        redirects,
        reduced_html,
    })
}

fn is_html(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.starts_with("text/html") || ct.starts_with("application/xhtml")
}

// --- web search --------------------------------------------------------------

/// One result row from `web_search`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// How many results a request may ask for, and how many it gets by default.
/// The cap is low on purpose: past the first page, a search result is a
/// context cost with almost no additional signal.
const SEARCH_DEFAULT_RESULTS: usize = 8;
const SEARCH_MAX_RESULTS: usize = 20;
const SEARCH_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Search the web through DuckDuckGo's HTML endpoint and return the results.
///
/// It goes through the same [`plan_raw`] / [`fetch`] path as `web_fetch`, so
/// the SSRF policy applies to a search exactly as it does to a direct fetch —
/// the request is not a special case that gets to skip the address check.
/// The endpoint is HTML rather than an API so the tool needs no key and no
/// account; the cost is that the parser is tied to the page's markup, which
/// is why an unrecognised page yields *no results* rather than a fabricated
/// answer.
pub fn search(query: &str, max_results: Option<u32>) -> Result<Vec<SearchResult>, Refusal> {
    let query = query.trim();
    if query.is_empty() {
        return Err(Refusal::InvalidUrl("the search query is empty".into()));
    }
    let want = match max_results {
        None | Some(0) => SEARCH_DEFAULT_RESULTS,
        Some(n) => (n as usize).min(SEARCH_MAX_RESULTS),
    };
    let mut url = Url::parse("https://html.duckduckgo.com/html/")
        .expect("the search endpoint is a valid literal");
    url.query_pairs_mut().append_pair("q", query);
    let plan = plan_raw(url.as_str(), Some(SEARCH_MAX_BYTES as u64))?;
    let page = fetch(&plan)?;
    Ok(parse_search(&page.body, want))
}

/// Extract the result rows from DuckDuckGo's HTML.
///
/// Split out from [`search`] so it can be tested without a network: this is
/// the part with the behaviour, and it is a pure function of the page text.
fn parse_search(html: &str, want: usize) -> Vec<SearchResult> {
    // DDG marks a result title with `class="result__a"` and its snippet with
    // `class="result__snippet"`. Matching the whole anchor, then reducing it,
    // keeps the extraction from depending on attribute order.
    let Ok(anchor) = Regex::new(r#"(?is)<a[^>]*class="[^"]*result__a[^"]*"[^>]*>.*?</a>"#) else {
        return Vec::new();
    };
    let Ok(snippet) = Regex::new(
        r#"(?is)<a[^>]*class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>"#,
    ) else {
        return Vec::new();
    };
    let Ok(href) = Regex::new(r#"(?is)href="([^"]+)""#) else {
        return Vec::new();
    };

    let mut snippets = snippet.captures_iter(html).map(|c| html_to_text(&c[1]));
    let mut out = Vec::new();
    for m in anchor.find_iter(html) {
        let tag = m.as_str();
        let Some(h) = href.captures(tag).map(|c| c[1].to_string()) else {
            continue;
        };
        let title = html_to_text(tag);
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        out.push(SearchResult {
            title: title.to_string(),
            url: decode_result_url(&h),
            snippet: snippets.next().unwrap_or_default().trim().to_string(),
        });
        if out.len() >= want {
            break;
        }
    }
    out
}

/// DDG wraps outbound links as `//duckduckgo.com/l/?uddg=<encoded>`. Unwrap
/// them back to the destination; leave anything else as it is (a protocol-
/// relative link gets its scheme back).
fn decode_result_url(href: &str) -> String {
    let absolute = if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        href.to_string()
    };
    let Ok(url) = Url::parse(&absolute) else {
        return href.to_string();
    };
    if !url.host_str().is_some_and(|h| h.ends_with("duckduckgo.com")) {
        return absolute;
    }
    match url.query_pairs().find(|(k, _)| k == "uddg") {
        Some((_, v)) => v.into_owned(),
        None => absolute,
    }
}

// --- HTML reduction ----------------------------------------------------------

/// Elements whose *contents* are not text and must not reach the reader:
/// usually hundreds of lines of code that would drown the page.
const DROPPED_ELEMENTS: &[&str] = &["script", "style", "noscript", "svg", "template"];

/// Elements whose closing tag ends a line of text. A scanner has no layout
/// engine, so without this a whole page arrives as one line — which is not
/// wrong, but is unreadable.
const BLOCK_ELEMENTS: &[&str] = &[
    "p", "div", "li", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "section", "article", "header",
    "footer", "table", "ul", "ol", "pre", "blockquote", "br", "hr", "form", "nav", "aside", "main",
    "dt", "dd", "figcaption", "figure",
];

/// Reduce HTML to readable text: drop the non-text subtrees, drop the tags,
/// decode the entities that matter, and collapse whitespace.
///
/// An approximation on purpose, and a small one: it is a single left-to-right
/// scan with no lookahead beyond the tag it is standing on. It will misread
/// a stray `<` in prose as the start of a tag — which is why the alternative,
/// pretending to parse HTML, was rejected: a scanner that is *obviously*
/// approximate is safer than a parser that is wrong in ways its author did
/// not think of.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len().min(4096));
    let mut rest = html;

    while let Some(lt) = rest.find('<') {
        push_text(&mut out, &rest[..lt]);
        let from_tag = &rest[lt..];

        // A comment runs to `-->`; a declaration (`<!doctype`) has no
        // separate close, so it ends at the next `>`. Anything unterminated is
        // the rest of the document, and is dropped.
        let end = if from_tag.starts_with("<!--") {
            from_tag.find("-->").map(|i| i + 3)
        } else {
            from_tag.find('>').map(|i| i + 1)
        };
        let Some(end) = end else {
            return finish(out);
        };
        let tag = &from_tag[..end];
        let tail = &from_tag[end..];

        let name = tag_name(tag);
        if let Some(dropped) = DROPPED_ELEMENTS.iter().find(|d| **d == name) {
            // Skip the subtree, not just the opening tag: the *contents* of a
            // script are the thing that must not reach the reader.
            rest = match find_close(tail, dropped) {
                Some(after) => after,
                None => return finish(out),
            };
        } else {
            if BLOCK_ELEMENTS.contains(&name.as_str()) || name == "br" {
                out.push('\n');
            }
            rest = tail;
        }
    }
    push_text(&mut out, rest);
    finish(out)
}

/// Append a run of character data, dropping a lone `>`.
///
/// Every `<` in character data has already been consumed as the start of a
/// tag, so a `>` that survives to here can only be a stray close-bracket the
/// author wrote literally instead of `&gt;`. It is markup noise, not text: a
/// browser renders it, but this scanner's contract is that no raw bracket
/// reaches the reader. Entities are still intact at this point (`&gt;` has
/// not been decoded yet), so the one legitimate `>` — the one the author
/// spelled as an entity — is preserved by [`decode_entities`].
fn push_text(out: &mut String, text: &str) {
    out.extend(text.chars().filter(|c| *c != '>'));
}

/// The lowercase element name of a tag, ignoring `</`, attributes, and any
/// self-closing slash. Empty for a comment or a declaration.
fn tag_name(tag: &str) -> String {
    let inner = tag
        .trim_start_matches('<')
        .trim_start_matches('/')
        .trim_end_matches('>')
        .trim_end_matches('/');
    inner
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Everything after `</name …>`, so the caller can resume there.
fn find_close<'a>(haystack: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("</{name}");
    let lowered = haystack.to_ascii_lowercase();
    let at = lowered.find(&needle)?;
    let after = &haystack[at..];
    after.find('>').map(|i| &after[i + 1..])
}

/// Decode entities and collapse whitespace.
fn finish(text: String) -> String {
    let decoded = decode_entities(&text);
    collapse_whitespace(&decoded)
}

/// The named entities worth decoding, plus numeric references. `&amp;` is
/// decoded **last** so that `&amp;lt;` becomes `&lt;` — the text the author
/// wrote — rather than `<`.
fn decode_entities(text: &str) -> String {
    let mut out = text.to_string();
    for (entity, replacement) in [
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&#39;", '\''),
        ("&apos;", '\''),
        ("&nbsp;", ' '),
        ("&amp;", '&'),
    ] {
        out = out.replace(entity, &replacement.to_string());
    }
    decode_numeric(&out)
}

/// `&#65;` and `&#x41;` — decimal and hexadecimal character references.
fn decode_numeric(text: &str) -> String {
    if !text.contains("&#") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("&#") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let (digits, radix, skip) = match after.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16, true),
            None => (after, 10, false),
        };
        let end = digits.find(';');
        let parsed = end.and_then(|e| {
            let body = &digits[..e];
            if body.is_empty() || body.len() > 8 {
                return None;
            }
            u32::from_str_radix(body, radix).ok()
        });
        match parsed.and_then(char::from_u32) {
            Some(c) => {
                out.push(c);
                // Past the `;`, plus the `x` when there was one.
                rest = &digits[end.unwrap() + 1..];
                let _ = skip;
            }
            None => {
                // Not a character reference after all — a bare `&#` in prose,
                // or a code point that is not a character. Left as written.
                out.push_str("&#");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Runs of spaces and tabs become one space; three or more newlines become
/// two. `\r` is dropped rather than collapsed, so a CRLF page does not end up
/// with a stray character on every line.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_newlines = 0usize;
    let mut pending_space = false;

    for c in text.chars() {
        match c {
            '\r' => {}
            '\n' => {
                pending_space = false;
                pending_newlines = (pending_newlines + 1).min(2);
            }
            c if c.is_whitespace() => pending_space = true,
            c => {
                if pending_newlines > 0 && !out.is_empty() {
                    for _ in 0..pending_newlines {
                        out.push('\n');
                    }
                } else if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_newlines = 0;
                pending_space = false;
                out.push(c);
            }
        }
    }
    while out.ends_with('\n') || out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the address policy --------------------------------------------------

    /// Every range the policy names, at both ends and one past each end, so a
    /// boundary that is off by one on either side is caught rather than
    /// sampled past. The addresses just outside a range are the ones that
    /// prove the range stops where it says.
    #[test]
    fn the_block_list_matches_its_ranges_exactly() {
        let blocked = [
            "169.254.0.0",
            "169.254.169.254",
            "169.254.255.255",
            "10.0.0.0",
            "10.255.255.255",
            "172.16.0.0",
            "172.31.255.255",
            "192.168.0.0",
            "192.168.255.255",
            "100.64.0.0",
            "100.127.255.255",
            "0.0.0.0",
            "224.0.0.1",
            "239.255.255.255",
            "255.255.255.255",
        ];
        for addr in blocked {
            let ip: IpAddr = addr.parse().unwrap();
            assert!(
                blocked_range(ip).is_some(),
                "{addr} must not be reachable from a URL"
            );
        }

        // The neighbours: one address on each side of each boundary. These
        // are ordinary public addresses and blocking them would be a bug in
        // the opposite direction.
        let allowed = [
            "169.253.255.255",
            "169.255.0.0",
            "9.255.255.255",
            "11.0.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "192.167.255.255",
            "192.169.0.0",
            "100.63.255.255",
            "100.128.0.0",
            "223.255.255.255",
            "1.1.1.1",
            "127.0.0.1",
            "127.255.255.254",
        ];
        for addr in allowed {
            let ip: IpAddr = addr.parse().unwrap();
            assert_eq!(
                blocked_range(ip),
                None,
                "{addr} is a real destination and must stay fetchable"
            );
        }
    }

    #[test]
    fn ipv6_is_covered_including_the_ipv4_it_wraps() {
        for addr in [
            "fe80::1",                // link-local
            "fc00::1",                // unique-local
            "fdff:ffff::1",           // the far end of unique-local
            "::",                     // unspecified
            "ff02::1",                // multicast
            "::ffff:169.254.169.254", // the metadata address, written as IPv6
            "::ffff:10.0.0.1",
            "::ffff:192.168.1.1",
        ] {
            let ip: IpAddr = addr.parse().unwrap();
            assert!(
                blocked_range(ip).is_some(),
                "{addr} must not be reachable from a URL"
            );
        }
        for addr in ["fec0::1", "2606:4700::1111", "::1", "::ffff:1.1.1.1"] {
            let ip: IpAddr = addr.parse().unwrap();
            assert_eq!(blocked_range(ip), None, "{addr} must stay fetchable");
        }
    }

    // --- the planner ---------------------------------------------------------

    #[test]
    fn only_http_and_https_leave_the_machine() {
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/x",
            "data:text/html,<b>x</b>",
            "javascript:alert(1)",
            "gopher://example.com/",
            "ws://example.com/",
        ] {
            let refused = plan(url, None).unwrap_err();
            assert!(
                matches!(refused, Refusal::UnsupportedScheme(_)),
                "{url}: {refused:?}"
            );
        }
    }

    #[test]
    fn malformed_urls_are_refused_at_the_plan() {
        for url in ["", "   ", "not a url", "http://", "example.com", "/"] {
            let refused = plan(url, None).unwrap_err();
            assert!(
                matches!(refused, Refusal::InvalidUrl(_)),
                "{url:?}: {refused:?}"
            );
        }

        // Credentials are refused rather than stripped: silently dropping them
        // would send an unauthenticated request the caller thinks is
        // authenticated, and forwarding them would leak them to every hop.
        let refused = plan("https://user:secret@example.com/", None).unwrap_err();
        assert_eq!(refused, Refusal::Credentials);
    }

    #[test]
    fn the_cap_is_a_ceiling_and_an_absent_one_is_a_default() {
        assert_eq!(plan("https://a.test/", None).unwrap().max_bytes, DEFAULT_BYTES);
        assert_eq!(plan("https://a.test/", Some(0)).unwrap().max_bytes, DEFAULT_BYTES);
        assert_eq!(plan("https://a.test/", Some(10)).unwrap().max_bytes, 10);
        assert_eq!(
            plan("https://a.test/", Some(u64::MAX)).unwrap().max_bytes,
            MAX_BYTES,
            "a caller cannot ask for an unbounded read"
        );
    }

    // --- HTML reduction ------------------------------------------------------

    #[test]
    fn html_reduction_drops_markup_and_keeps_text() {
        let page = "<html><head><title>T</title><style>body{color:red}</style>\
                    <script>var x = '<b>not text</b>';</script></head>\
                    <body><h1>Hello</h1><p>World &amp; friends</p>\
                    <ul><li>one</li><li>two</li></ul></body></html>";
        let text = html_to_text(page);
        assert!(text.contains("Hello"), "{text:?}");
        assert!(text.contains("World & friends"), "{text:?}");
        assert!(text.contains("one"), "{text:?}");
        assert!(
            !text.contains("color:red") && !text.contains("not text"),
            "script and style contents are not text: {text:?}"
        );
        assert!(!text.contains('<') && !text.contains('>'), "{text:?}");
        // Block elements end a line, so the page is not one long string.
        assert!(text.lines().count() > 3, "{text:?}");
    }

    #[test]
    fn search_parses_titles_links_and_snippets() {
        // The shape DDG's HTML endpoint returns, including the wrapped
        // outbound link and an entity in the title.
        let html = r#"
            <div class="result">
              <h2><a rel="nofollow" class="result__a"
                     href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&amp;rut=z">Rust &amp; Cargo</a></h2>
              <a class="result__snippet" href="x">The <b>book</b>.</a>
            </div>
            <div class="result">
              <h2><a class="result__a" href="https://rust-lang.org/">Rust</a></h2>
              <a class="result__snippet">Home page.</a>
            </div>
        "#;
        let results = parse_search(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust & Cargo");
        assert_eq!(results[0].url, "https://example.com/a");
        assert_eq!(results[0].snippet, "The book.");
        assert_eq!(results[1].title, "Rust");
        assert_eq!(results[1].url, "https://rust-lang.org/");
        assert_eq!(results[1].snippet, "Home page.");
        // The cap is what stops the walk, exactly as in grep.
        assert_eq!(parse_search(html, 1).len(), 1);
    }

    #[test]
    fn a_page_with_no_results_yields_none_rather_than_a_guess() {
        assert!(parse_search("<html><body>no results here</body></html>", 8).is_empty());
    }

    #[test]
    fn entities_decode_and_a_double_escaped_ampersand_stays_escaped() {
        assert_eq!(html_to_text("a &lt;b&gt; c"), "a <b> c");
        assert_eq!(html_to_text("&quot;quoted&quot;"), "\"quoted\"");
        assert_eq!(html_to_text("x&nbsp;y"), "x y");
        assert_eq!(html_to_text("&#65;&#x42;"), "AB");
        // `&amp;lt;` is the *text* `&lt;`: decoding `&amp;` first would turn it
        // into a tag the reader never wrote.
        assert_eq!(html_to_text("&amp;lt;"), "&lt;");
        // Not a character reference, and not a reason to drop anything.
        assert_eq!(html_to_text("100 &# 200"), "100 &# 200");
        assert_eq!(html_to_text("&#x110000;"), "&#x110000;");
    }

    #[test]
    fn whitespace_collapses_without_losing_paragraphs() {
        assert_eq!(collapse_whitespace("a  \t b"), "a b");
        assert_eq!(collapse_whitespace("a\n\n\n\n\nb"), "a\n\nb");
        assert_eq!(collapse_whitespace("a\r\nb"), "a\nb");
        assert_eq!(collapse_whitespace("  \n  hello  \n  "), "hello");
        assert_eq!(collapse_whitespace(""), "");
    }

    /// Exhaustive over a small alphabet rather than sampled.
    ///
    /// The scanner's whole risk is a `<` in a position it did not anticipate,
    /// and the alphabet that can express those positions is tiny — so every
    /// string of length ≤ 4 over `{<, >, /, a, ;, space}` is checked, which is
    /// a proof about that domain rather than evidence about a sample of it.
    #[test]
    fn the_scanner_never_panics_and_never_leaves_a_tag() {
        let alphabet = ['<', '>', '/', 'a', ';', ' '];
        let mut checked = 0usize;
        for len in 0..=4 {
            let mut indices = vec![0usize; len];
            loop {
                let input: String = indices.iter().map(|i| alphabet[*i]).collect();
                let out = html_to_text(&input);
                // No input here contains `&`, so no entity can decode into a
                // bracket: any `<` or `>` left in the output is a tag the
                // scanner failed to drop.
                assert!(
                    !out.contains('<') && !out.contains('>'),
                    "{input:?} left markup behind: {out:?}"
                );
                assert!(out.len() <= input.len().max(1), "{input:?} grew: {out:?}");
                checked += 1;

                // Odometer over the alphabet.
                let mut pos = len;
                loop {
                    if pos == 0 {
                        break;
                    }
                    pos -= 1;
                    indices[pos] += 1;
                    if indices[pos] < alphabet.len() {
                        break;
                    }
                    indices[pos] = 0;
                    if pos == 0 {
                        break;
                    }
                }
                if len == 0 || indices.iter().all(|i| *i == 0) {
                    break;
                }
            }
        }
        assert_eq!(checked, 1 + 6 + 36 + 216 + 1296, "the whole domain");
    }
}
