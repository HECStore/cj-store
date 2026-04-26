//! Web tools — `web_fetch` with hardened SSRF defenses.
//!
//! The two pieces with security weight live here as pure functions so
//! they're trivially testable:
//!
//! 1. [`validate_url`] — reject non-`http(s)` schemes, numeric-form
//!    hostnames (decimal `2130706433`, octal, hex), zero-page hosts,
//!    userinfo (`user@host`).
//! 2. [`is_denied_ip`] — IP deny-list for resolved addresses, including
//!    cloud metadata IPs (`169.254.169.254`).
//!
//! The actual HTTP fetch needs a custom `reqwest::dns::Resolve` impl
//! that pins connect-IP to the validated address (defeats DNS rebinding
//! TOCTOU). That goes in the chat task once the operator opts in
//! (`chat.web_fetch_enabled = true`); the primitives here are what it
//! consults.
//!
//! `web_search` is the Anthropic-managed server tool
//! (`web_search_20250305`); when available on the account it requires
//! no SSRF defense from us — Anthropic handles the network.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// A URL the SSRF primitives accept. Only constructible via
/// [`validate_url`], which establishes the scheme, host shape, and
/// userinfo invariants.
#[derive(Debug, Clone)]
pub struct SafeUrl {
    pub scheme: Scheme,
    pub host: Host,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// A textual hostname that still needs DNS resolution.
    Name(String),
    /// A literal IPv4/IPv6 in the URL — the resolver step is skipped
    /// and the deny-list is consulted directly.
    Ip(IpAddr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    /// Scheme is not `http` or `https`.
    BadScheme,
    /// URL contains userinfo (e.g. `http://user@host/`).
    HasUserinfo,
    /// Hostname is in numeric form (decimal, octal, hex) which is
    /// commonly used to obscure metadata-IP addresses.
    NumericHostname,
    /// Hostname is empty / parseable but malformed.
    BadHost,
    /// Generic parse error — URL did not look like an http(s) URL.
    BadFormat,
}

/// Validate a URL string first three bullets. Performs
/// no DNS resolution; the resolved-IP deny-list runs in
/// [`is_denied_ip`] after `tokio::net::lookup_host` (or the custom
/// resolver) returns.
pub fn validate_url(input: &str) -> Result<SafeUrl, UrlError> {
    // Reject control chars and whitespace up front.
    if input
        .chars()
        .any(|c| c.is_control() || c == ' ' || c == '\t')
    {
        return Err(UrlError::BadFormat);
    }
    // Scheme split.
    let lower = input.to_lowercase();
    let (scheme, rest) = if let Some(rest) = lower.strip_prefix("https://") {
        (Scheme::Https, &input[8..8 + rest.len()])
    } else if let Some(rest) = lower.strip_prefix("http://") {
        (Scheme::Http, &input[7..7 + rest.len()])
    } else {
        return Err(UrlError::BadScheme);
    };

    // Userinfo: `user[:pass]@host…`. `@` before the first `/` is
    // userinfo. (Path can also contain `@` — only count those before
    // the path separator.)
    let path_start = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..path_start];
    if authority.contains('@') {
        return Err(UrlError::HasUserinfo);
    }
    if authority.is_empty() {
        return Err(UrlError::BadHost);
    }

    // IPv6 literal — `[ipv6]` or `[ipv6]:port`. Must handle BEFORE
    // the port-strip rsplit, otherwise `[2001:db8::1]:8080` would
    // split on the wrong colon and lose the closing bracket.
    if let Some(rest) = authority.strip_prefix('[') {
        let close = rest.find(']').ok_or(UrlError::BadHost)?;
        let inner = &rest[..close];
        let after = &rest[close + 1..];
        // After the bracket there may be `:port` or nothing.
        if !after.is_empty()
            && (!after.starts_with(':') || after[1..].is_empty()
                || !after[1..].bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(UrlError::BadHost);
        }
        return inner
            .parse::<Ipv6Addr>()
            .map(|ip| SafeUrl {
                scheme,
                host: Host::Ip(IpAddr::V6(ip)),
                url: input.to_string(),
            })
            .map_err(|_| UrlError::BadHost);
    }

    // Strip optional `:port` (single trailing colon-decimal).
    let host_str = match authority.rsplit_once(':') {
        Some((h, port))
            if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) =>
        {
            h
        }
        _ => authority,
    };
    if host_str.is_empty() {
        return Err(UrlError::BadHost);
    }

    // IPv4 literal? Reject decimal, octal, hex obfuscation forms; ONLY
    // accept dotted-quad with each octet in 0-255 written in decimal
    // with no leading zeros (`0177.0.0.1` is rejected).
    if let Some(ip) = parse_strict_dotted_quad(host_str) {
        return Ok(SafeUrl {
            scheme,
            host: Host::Ip(IpAddr::V4(ip)),
            url: input.to_string(),
        });
    }
    // If the host LOOKS like a numeric IP address in any obfuscated
    // form (single integer in decimal/octal/hex; or dotted-quad with
    // leading-zero octets), it must NOT be re-treated as a hostname.
    if looks_numeric_hostname(host_str) || looks_obfuscated_dotted_quad(host_str) {
        return Err(UrlError::NumericHostname);
    }
    // Otherwise it's a textual hostname.
    if !looks_like_hostname(host_str) {
        return Err(UrlError::BadHost);
    }
    Ok(SafeUrl {
        scheme,
        host: Host::Name(host_str.to_lowercase()),
        url: input.to_string(),
    })
}

/// Detect dotted-quad-with-obfuscation: 4 dot-separated parts that each
/// look numeric but include leading-zero / oversize octets that
/// [`parse_strict_dotted_quad`] rejected. Catches forms like
/// `0177.0.0.1`.
fn looks_obfuscated_dotted_quad(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts.iter().all(|p| {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit() || b == b'x' || b == b'X')
    })
}

fn parse_strict_dotted_quad(s: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut octets = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || p.len() > 3 {
            return None;
        }
        // Reject leading-zero octets (often used in obfuscation).
        if p.len() > 1 && p.starts_with('0') {
            return None;
        }
        if !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let val: u32 = p.parse().ok()?;
        if val > 255 {
            return None;
        }
        octets[i] = val as u8;
    }
    Some(Ipv4Addr::from(octets))
}

fn looks_numeric_hostname(s: &str) -> bool {
    // Pure decimal integer: e.g. `2130706433` (which is `127.0.0.1`).
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    // Hex: `0x7f000001`.
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))
        && !rest.is_empty()
        && rest.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return true;
    }
    // Octal: leading `0` followed by digits 0-7 only (excludes the
    // strict-quad forms because those have at most 3 chars per octet).
    if s.len() > 1 && s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    false
}

fn looks_like_hostname(s: &str) -> bool {
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    // Each label 1-63 chars; alnum + hyphen; not starting/ending with hyphen.
    for label in s.split('.') {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return false;
        }
    }
    true
}

/// IP deny-list. Returns `true` if the resolved IP MUST
/// be rejected before connecting.
pub fn is_denied_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_denied_ipv4(v4),
        IpAddr::V6(v6) => is_denied_ipv6(v6),
    }
}

fn is_denied_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    // 0.0.0.0/8 — current network / "this host" — used in some SSRF
    // exploits to reach the local interface.
    if o[0] == 0 {
        return true;
    }
    // 127.0.0.0/8 — loopback.
    if o[0] == 127 {
        return true;
    }
    // 10.0.0.0/8 — private.
    if o[0] == 10 {
        return true;
    }
    // 172.16.0.0/12 — private.
    if o[0] == 172 && (16..=31).contains(&o[1]) {
        return true;
    }
    // 192.168.0.0/16 — private.
    if o[0] == 192 && o[1] == 168 {
        return true;
    }
    // 169.254.0.0/16 — link-local incl. AWS/GCP metadata 169.254.169.254.
    if o[0] == 169 && o[1] == 254 {
        return true;
    }
    // 100.64.0.0/10 — CGNAT.
    if o[0] == 100 && (64..=127).contains(&o[1]) {
        return true;
    }
    // 224.0.0.0/4 — multicast.
    if o[0] >= 224 {
        return true;
    }
    // 255.255.255.255 — broadcast (covered by /4 above but explicit).
    if o == [255, 255, 255, 255] {
        return true;
    }
    false
}

fn is_denied_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    let segs = ip.segments();
    // ULA fc00::/7 — unique local addresses.
    if (segs[0] & 0xfe00) == 0xfc00 {
        return true;
    }
    // Link-local fe80::/10.
    if (segs[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    // 64:ff9b::/96 — IPv4/IPv6 well-known prefix; treat conservatively.
    if segs[0] == 0x0064 && segs[1] == 0xff9b && segs[2] == 0 && segs[3] == 0 && segs[4] == 0 && segs[5] == 0 {
        return true;
    }
    // ::ffff:0:0/96 — IPv4-mapped IPv6: validate the embedded IPv4.
    if let Some(v4) = ip.to_ipv4_mapped()
        && is_denied_ipv4(v4)
    {
        return true;
    }
    false
}

/// Hostnames the deny-list must reject regardless of resolution. The
/// most common is GCP's metadata DNS name, which resolves to
/// 169.254.169.254 (already in the IP deny-list) — but a hostile
/// resolver could lie. CHAT.md
pub fn is_denied_hostname(hostname: &str) -> bool {
    let lc = hostname.to_lowercase();
    matches!(
        lc.as_str(),
        "metadata.google.internal" | "metadata"
    )
}

// ===== Live fetch ==========================================================

use std::net::SocketAddr;

const FETCH_TIMEOUT_SECS: u64 = 5;
const MAX_REDIRECTS: u8 = 3;

/// Fetch a URL with full SSRF mitigations:
///
/// - URL parse + scheme + userinfo + numeric-host rejection.
/// - DNS resolution → IP deny-list.
/// - Connect-IP pinning via `reqwest::ClientBuilder::resolve_to_addrs`.
/// - Redirects disabled at the reqwest level; we follow up to 3 hops
///   manually, re-validating the `Location` URL each time.
/// - Streaming body read with a running byte counter; aborts on cap.
/// - Rejects `Content-Encoding` other than identity (zip-bomb defense).
///
/// Returns the response body as plain text (stripped of HTML tags) or
/// an error string suitable for the model's tool_result.
pub async fn fetch(url: &str, max_bytes: usize) -> Result<String, String> {
    let mut current = url.to_string();
    for hop in 0..=MAX_REDIRECTS {
        let safe = validate_url(&current).map_err(|e| format!("url rejected: {e:?}"))?;
        let host_for_check = match &safe.host {
            Host::Name(n) => n.clone(),
            Host::Ip(_) => String::new(),
        };
        if !host_for_check.is_empty() && is_denied_hostname(&host_for_check) {
            return Err("hostname is on the deny-list".to_string());
        }

        // Resolve hostname → vetted IPs.
        let resolved_addrs: Vec<SocketAddr> = match &safe.host {
            Host::Ip(ip) => {
                if is_denied_ip(*ip) {
                    return Err("resolved IP is on the deny-list".to_string());
                }
                let port = port_for(&safe);
                vec![SocketAddr::new(*ip, port)]
            }
            Host::Name(name) => {
                let port = port_for(&safe);
                let lookup = format!("{name}:{port}");
                let addrs: Vec<SocketAddr> = tokio::net::lookup_host(lookup)
                    .await
                    .map_err(|e| format!("DNS lookup failed: {e}"))?
                    .collect();
                let mut accepted = Vec::with_capacity(addrs.len());
                for a in addrs {
                    if is_denied_ip(a.ip()) {
                        continue;
                    }
                    accepted.push(a);
                }
                if accepted.is_empty() {
                    return Err("all resolved IPs are on the deny-list".to_string());
                }
                accepted
            }
        };

        let host_str = match &safe.host {
            Host::Name(n) => n.clone(),
            Host::Ip(ip) => ip.to_string(),
        };

        // Build a fresh reqwest client per call: we pin the resolver
        // to the validated IPs, which is a per-host config.
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none())
            .https_only(false); // operator may explicitly fetch http URLs
        for addr in &resolved_addrs {
            builder = builder.resolve_to_addrs(&host_str, &[*addr]);
        }
        let client = builder
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;

        let resp = client
            .get(&safe.url)
            .send()
            .await
            .map_err(|e| format!("fetch failed: {e}"))?;

        // Operator-visible log of every hop (initial + each redirect).
        // Deliberately host+status only — never path/query, so secrets
        // embedded in URLs cannot leak via logs.
        tracing::info!(
            host = %host_str,
            status = resp.status().as_u16(),
            "[Chat] web_fetch"
        );

        let status = resp.status();
        // Manual redirect handling — extract Location, re-validate.
        if status.is_redirection() {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let Some(loc) = loc else {
                return Err(format!("redirect status {status} without Location header"));
            };
            if hop >= MAX_REDIRECTS {
                return Err(format!("exceeded {MAX_REDIRECTS} redirects"));
            }
            // Resolve relative redirect against current URL.
            current = resolve_relative(&safe.url, &loc)?;
            continue;
        }
        if !status.is_success() {
            return Err(format!("fetch returned status {status}"));
        }

        // Defend against decompression-bomb: refuse non-identity encodings.
        if let Some(enc) = resp.headers().get(reqwest::header::CONTENT_ENCODING) {
            let v = enc.to_str().unwrap_or("");
            if !v.is_empty() && !v.eq_ignore_ascii_case("identity") {
                return Err(format!("rejected Content-Encoding '{v}'"));
            }
        }
        // Streaming body read with running byte counter.
        let mut body_bytes: Vec<u8> = Vec::with_capacity(8192);
        let mut total = 0usize;
        let mut stream = resp;
        loop {
            match stream.chunk().await {
                Ok(Some(chunk)) => {
                    total = total.saturating_add(chunk.len());
                    if total > max_bytes {
                        return Err(format!(
                            "response body exceeded {max_bytes} bytes"
                        ));
                    }
                    body_bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(e) => return Err(format!("body chunk error: {e}")),
            }
        }
        // CHAT.md: plain-text-only — strip HTML tags before returning.
        let text = String::from_utf8_lossy(&body_bytes).to_string();
        return Ok(strip_html_tags(&text));
    }
    Err("redirect loop terminated unexpectedly".to_string())
}

fn port_for(safe: &SafeUrl) -> u16 {
    // Parse port from URL if present, else default per scheme.
    // SafeUrl carries the original `url` string — quickest path is to
    // re-extract the port from there.
    let url = &safe.url;
    let scheme_len = match safe.scheme {
        Scheme::Http => 7,
        Scheme::Https => 8,
    };
    let after_scheme = &url[scheme_len..];
    let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
    let authority = &after_scheme[..path_start];
    // Strip IPv6 brackets if present.
    let port_part = if let Some(rest) = authority.strip_prefix('[') {
        if let Some(close) = rest.find(']') {
            let after_close = &rest[close + 1..];
            after_close.strip_prefix(':')
        } else {
            None
        }
    } else {
        authority.rsplit_once(':').map(|(_, p)| p)
    };
    if let Some(p) = port_part
        && let Ok(n) = p.parse()
    {
        return n;
    }
    match safe.scheme {
        Scheme::Http => 80,
        Scheme::Https => 443,
    }
}

fn resolve_relative(base: &str, location: &str) -> Result<String, String> {
    // Absolute URL — pass through; will be re-validated.
    if location.starts_with("http://") || location.starts_with("https://") {
        return Ok(location.to_string());
    }
    // Path-absolute or path-relative — splice onto the base authority.
    let scheme_len = if base.starts_with("https://") { 8 }
        else if base.starts_with("http://") { 7 }
        else { return Err("base URL has no http(s) scheme".to_string()); };
    let after = &base[scheme_len..];
    let path_start = after.find('/').unwrap_or(after.len());
    let authority = &after[..path_start];
    let scheme = &base[..scheme_len];
    if let Some(stripped) = location.strip_prefix('/') {
        Ok(format!("{scheme}{authority}/{stripped}"))
    } else {
        // Path-relative — resolve against last `/` of the base path.
        let base_path = &after[path_start..];
        let last_slash = base_path.rfind('/').unwrap_or(0);
        let prefix = &base_path[..=last_slash];
        Ok(format!("{scheme}{authority}{prefix}{location}"))
    }
}

/// Naive HTML tag stripper. Good enough for tool_result inclusion;
/// refusing to be a full HTML parser.
pub fn strip_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        if in_tag {
            if c == '>' {
                in_tag = false;
            }
        } else if c == '<' {
            in_tag = true;
        } else {
            out.push(c);
        }
    }
    // Collapse whitespace runs.
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_ws = false;
    for c in out.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                collapsed.push(' ');
                prev_ws = true;
            }
        } else {
            collapsed.push(c);
            prev_ws = false;
        }
    }
    collapsed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- validate_url ---------------------------------------------------

    #[test]
    fn https_with_textual_host_accepted() {
        let u = validate_url("https://example.com/path").unwrap();
        assert_eq!(u.scheme, Scheme::Https);
        assert!(matches!(u.host, Host::Name(ref n) if n == "example.com"));
    }

    #[test]
    fn http_with_port_accepted() {
        let u = validate_url("http://example.com:8080/").unwrap();
        assert!(matches!(u.host, Host::Name(ref n) if n == "example.com"));
    }

    #[test]
    fn ipv4_dotted_quad_accepted() {
        let u = validate_url("http://1.2.3.4/").unwrap();
        assert!(matches!(u.host, Host::Ip(IpAddr::V4(_))));
    }

    #[test]
    fn ipv6_literal_accepted_in_brackets() {
        let u = validate_url("http://[2001:db8::1]/").unwrap();
        assert!(matches!(u.host, Host::Ip(IpAddr::V6(_))));
    }

    #[test]
    fn rejects_non_http_scheme() {
        assert_eq!(validate_url("ftp://example.com/").unwrap_err(), UrlError::BadScheme);
        assert_eq!(validate_url("file:///etc/passwd").unwrap_err(), UrlError::BadScheme);
        assert_eq!(validate_url("javascript:alert(1)").unwrap_err(), UrlError::BadScheme);
        assert_eq!(validate_url("gopher://example.com/").unwrap_err(), UrlError::BadScheme);
    }

    #[test]
    fn rejects_userinfo() {
        assert_eq!(
            validate_url("http://user@example.com/").unwrap_err(),
            UrlError::HasUserinfo
        );
        assert_eq!(
            validate_url("http://user:pass@example.com/").unwrap_err(),
            UrlError::HasUserinfo
        );
    }

    #[test]
    fn rejects_decimal_numeric_hostname() {
        // 2130706433 = 127.0.0.1.
        assert_eq!(
            validate_url("http://2130706433/").unwrap_err(),
            UrlError::NumericHostname
        );
    }

    #[test]
    fn rejects_hex_numeric_hostname() {
        assert_eq!(
            validate_url("http://0x7f000001/").unwrap_err(),
            UrlError::NumericHostname
        );
    }

    #[test]
    fn rejects_octal_numeric_hostname() {
        // Octal-looking host (017700000001 = 127.0.0.1).
        assert_eq!(
            validate_url("http://017700000001/").unwrap_err(),
            UrlError::NumericHostname
        );
    }

    #[test]
    fn rejects_dotted_quad_with_leading_zero_octets() {
        // 0177.0.0.1 — octal-style obfuscation of 127.0.0.1.
        let r = validate_url("http://0177.0.0.1/");
        assert!(r.is_err());
    }

    #[test]
    fn rejects_url_with_whitespace() {
        assert_eq!(validate_url("http://exa mple.com/").unwrap_err(), UrlError::BadFormat);
        assert_eq!(validate_url("http://example.com/\nfoo").unwrap_err(), UrlError::BadFormat);
    }

    #[test]
    fn rejects_empty_host() {
        assert!(validate_url("http:///path").is_err());
    }

    // ---- is_denied_ip --------------------------------------------------

    #[test]
    fn loopback_v4_denied() {
        assert!(is_denied_ip("127.0.0.1".parse().unwrap()));
        assert!(is_denied_ip("127.255.255.254".parse().unwrap()));
    }

    #[test]
    fn loopback_v6_denied() {
        assert!(is_denied_ip("::1".parse().unwrap()));
    }

    #[test]
    fn rfc1918_v4_denied() {
        for s in ["10.0.0.1", "10.255.255.255", "172.16.0.1", "172.31.255.255", "192.168.1.1"] {
            assert!(is_denied_ip(s.parse().unwrap()), "should be denied: {s}");
        }
    }

    #[test]
    fn link_local_v4_denied_including_cloud_metadata() {
        assert!(is_denied_ip("169.254.0.1".parse().unwrap()));
        // 169.254.169.254 is the AWS / GCP metadata IP.
        assert!(is_denied_ip("169.254.169.254".parse().unwrap()));
    }

    #[test]
    fn cgnat_v4_denied() {
        assert!(is_denied_ip("100.64.0.1".parse().unwrap()));
        assert!(is_denied_ip("100.127.255.254".parse().unwrap()));
    }

    #[test]
    fn multicast_v4_denied() {
        assert!(is_denied_ip("224.0.0.1".parse().unwrap()));
        assert!(is_denied_ip("239.255.255.255".parse().unwrap()));
    }

    #[test]
    fn zero_network_v4_denied() {
        assert!(is_denied_ip("0.0.0.0".parse().unwrap()));
        assert!(is_denied_ip("0.255.0.1".parse().unwrap()));
    }

    #[test]
    fn ipv4_mapped_v6_consults_v4_denylist() {
        // ::ffff:127.0.0.1 — same loopback, IPv6-mapped form.
        assert!(is_denied_ip("::ffff:127.0.0.1".parse().unwrap()));
        // ::ffff:8.8.8.8 — public, allowed.
        assert!(!is_denied_ip("::ffff:8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn ula_v6_denied() {
        assert!(is_denied_ip("fd00::1".parse().unwrap()));
    }

    #[test]
    fn link_local_v6_denied() {
        assert!(is_denied_ip("fe80::1".parse().unwrap()));
    }

    #[test]
    fn public_v4_allowed() {
        // Public DNS resolvers — known good test addresses.
        assert!(!is_denied_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_denied_ip("1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn public_v6_allowed() {
        assert!(!is_denied_ip("2001:4860:4860::8888".parse().unwrap()));
    }

    // ---- denied hostnames ----------------------------------------------

    #[test]
    fn metadata_hostname_denied() {
        assert!(is_denied_hostname("metadata.google.internal"));
        assert!(is_denied_hostname("METADATA.GOOGLE.INTERNAL"));
        assert!(is_denied_hostname("metadata"));
    }

    #[test]
    fn ordinary_hostnames_allowed() {
        assert!(!is_denied_hostname("example.com"));
        assert!(!is_denied_hostname("api.anthropic.com"));
    }

    // ---- helpers --------------------------------------------------------

    #[test]
    fn strip_html_tags_basic() {
        let html = "<p>Hello <b>world</b>!</p>";
        assert_eq!(strip_html_tags(html), "Hello world!");
    }

    #[test]
    fn strip_html_tags_collapses_whitespace() {
        let html = "<div>\n   one\n  two   three   </div>";
        assert_eq!(strip_html_tags(html), "one two three");
    }

    #[test]
    fn strip_html_tags_handles_attributes() {
        let html = r#"<a href="https://example.com" class="x">link</a>"#;
        assert_eq!(strip_html_tags(html), "link");
    }

    #[test]
    fn resolve_relative_absolute_passes_through() {
        let r = resolve_relative("https://a.com/x", "https://b.com/y").unwrap();
        assert_eq!(r, "https://b.com/y");
    }

    #[test]
    fn resolve_relative_root_path() {
        let r = resolve_relative("https://a.com/path/here", "/other").unwrap();
        assert_eq!(r, "https://a.com/other");
    }

    #[test]
    fn resolve_relative_relative_path() {
        let r = resolve_relative("https://a.com/path/here", "next").unwrap();
        assert_eq!(r, "https://a.com/path/next");
    }

    #[test]
    fn port_for_uses_default_when_unspecified() {
        let safe = SafeUrl {
            scheme: Scheme::Https,
            host: Host::Name("example.com".to_string()),
            url: "https://example.com/x".to_string(),
        };
        assert_eq!(port_for(&safe), 443);
        let safe = SafeUrl {
            scheme: Scheme::Http,
            host: Host::Name("example.com".to_string()),
            url: "http://example.com/".to_string(),
        };
        assert_eq!(port_for(&safe), 80);
    }

    #[test]
    fn port_for_extracts_explicit_port() {
        let safe = SafeUrl {
            scheme: Scheme::Http,
            host: Host::Name("example.com".to_string()),
            url: "http://example.com:8080/path".to_string(),
        };
        assert_eq!(port_for(&safe), 8080);
    }
}
