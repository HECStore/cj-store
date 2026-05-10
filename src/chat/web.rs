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

/// Egress port allow-list. SSRF defenses gate not just on resolved IP
/// but also on destination port — a public host like
/// `victim.example.com:6379` would otherwise let an attacker reach an
/// internal-protocol service (Redis, Ollama, SMTP, …) via a permitted
/// IP. Only the standard HTTP(S) ports are accepted.
const ALLOWED_PORTS: &[u16] = &[80, 443];

/// A URL the SSRF primitives accept. Only constructible via
/// [`validate_url`], which establishes the scheme, host shape, and
/// userinfo invariants.
#[derive(Debug, Clone)]
pub struct SafeUrl {
    pub scheme: Scheme,
    pub host: Host,
    pub url: String,
    /// Effective TCP port — explicit `:port` if present in the URL,
    /// otherwise the scheme default (80 for http, 443 for https). Stored
    /// at construction time so the port is a single source of truth that
    /// the connect path can trust without re-parsing the URL string.
    pub port: u16,
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
    /// Explicit port is not on the [`ALLOWED_PORTS`] allow-list.
    DisallowedPort(u16),
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlError::BadScheme => write!(f, "scheme is not http or https"),
            UrlError::HasUserinfo => write!(f, "URL contains userinfo"),
            UrlError::NumericHostname => write!(f, "hostname is in numeric form"),
            UrlError::BadHost => write!(f, "hostname is empty or malformed"),
            UrlError::BadFormat => write!(f, "URL is malformed"),
            UrlError::DisallowedPort(p) => {
                write!(f, "port {p} is not in the allow-list (only 80, 443 permitted)")
            }
        }
    }
}

/// Validate a URL string first three bullets. Performs
/// no DNS resolution; the resolved-IP deny-list runs in
/// [`is_denied_ip`] after `tokio::net::lookup_host` (or the custom
/// resolver) returns.
pub fn validate_url(input: &str) -> Result<SafeUrl, UrlError> {
    // Hard URL length cap — defends against DoS-on-cheap-input. The
    // prompt-injection-driven huge URL construction case (a model coaxed
    // into emitting a multi-megabyte URL) would otherwise burn allocator
    // bandwidth before any validation gate fires. RFC 7230 §3.1.1
    // recommends servers support 8000 bytes; an 8192-byte cap leaves a
    // small margin without permitting anything pathological.
    if input.len() > 8192 {
        return Err(UrlError::BadFormat);
    }
    // Reject control chars and whitespace up front. `c.is_control()` only
    // covers Unicode Cc (C0/C1 control bytes); explicitly add the Cf
    // format chars that are invisible in editor / log output and could
    // smuggle authority changes past a casual review: zero-width space
    // U+200B, BOM U+FEFF, LTR/RTL marks U+200E/U+200F, line/paragraph
    // separators U+2028/U+2029. The host portion already requires ASCII
    // via `looks_like_hostname` / IP-literal forms, so this gate makes
    // the contract explicit for the whole input string.
    if input.chars().any(|c| {
        c.is_control()
            || c == ' '
            || c == '\t'
            || matches!(
                c,
                '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{200E}' | '\u{200F}'
                    | '\u{2028}' | '\u{2029}' | '\u{FEFF}'
            )
    }) {
        return Err(UrlError::BadFormat);
    }
    // Scheme split. Done via byte-level ASCII-case-insensitive prefix
    // match so a URL containing Unicode that changes byte length on
    // lowercase (e.g. Turkish 'İ' → "i\u{307}") cannot break the
    // length-based slice and panic. The first 7-8 bytes of any valid
    // http(s) URL are ASCII, so slicing at byte 7/8 is char-aligned.
    let bytes = input.as_bytes();
    let (scheme, rest) = if bytes.len() >= 8
        && bytes[..8].eq_ignore_ascii_case(b"https://")
    {
        (Scheme::Https, &input[8..])
    } else if bytes.len() >= 7
        && bytes[..7].eq_ignore_ascii_case(b"http://")
    {
        (Scheme::Http, &input[7..])
    } else {
        return Err(UrlError::BadScheme);
    };

    // Userinfo: `user[:pass]@host…`. `@` before the first authority
    // terminator is userinfo. (Path / query / fragment can also contain
    // `@` — only count those before the authority ends.)
    //
    // The authority ends at whichever comes first: `/` (path), `?`
    // (query), or `#` (fragment). URLs with no path but a query or
    // fragment (e.g. `http://example.com?x=1`, common for SPA routes
    // and search APIs) must split on `?`/`#` here, otherwise the
    // query/fragment text would be glommed onto the host string.
    let path_start = rest
        .find(|c: char| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
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
        // Gate the explicit port (if any) against the allow-list. An
        // implicit (absent) port is fine — default-per-scheme below.
        let mut effective_port = default_port_for_scheme(scheme);
        if let Some(port_str) = after.strip_prefix(':') {
            let p: u16 = port_str.parse().map_err(|_| UrlError::BadHost)?;
            if !ALLOWED_PORTS.contains(&p) {
                return Err(UrlError::DisallowedPort(p));
            }
            effective_port = p;
        }
        return inner
            .parse::<Ipv6Addr>()
            .map(|ip| SafeUrl {
                scheme,
                host: Host::Ip(IpAddr::V6(ip)),
                url: input.to_string(),
                port: effective_port,
            })
            .map_err(|_| UrlError::BadHost);
    }

    // Strip optional `:port` (single trailing colon-decimal).
    let (host_str, explicit_port) = match authority.rsplit_once(':') {
        Some((h, port))
            if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) =>
        {
            let p: u16 = port.parse().map_err(|_| UrlError::BadHost)?;
            (h, Some(p))
        }
        _ => (authority, None),
    };
    if host_str.is_empty() {
        return Err(UrlError::BadHost);
    }
    // Gate the explicit port (if any) against the allow-list. An
    // implicit (absent) port is fine — `port_for` will substitute
    // 80/443 per scheme, both of which are on the list.
    if let Some(p) = explicit_port
        && !ALLOWED_PORTS.contains(&p)
    {
        return Err(UrlError::DisallowedPort(p));
    }

    let effective_port = explicit_port.unwrap_or_else(|| default_port_for_scheme(scheme));

    // IPv4 literal? Reject decimal, octal, hex obfuscation forms; ONLY
    // accept dotted-quad with each octet in 0-255 written in decimal
    // with no leading zeros (`0177.0.0.1` is rejected).
    if let Some(ip) = parse_strict_dotted_quad(host_str) {
        return Ok(SafeUrl {
            scheme,
            host: Host::Ip(IpAddr::V4(ip)),
            url: input.to_string(),
            port: effective_port,
        });
    }
    // If the host LOOKS like a numeric IP address in any obfuscated
    // form (single integer in decimal/octal/hex; or dotted-quad with
    // leading-zero octets), it must NOT be re-treated as a hostname.
    if looks_numeric_hostname(host_str)
        || looks_obfuscated_dotted_quad(host_str)
        || looks_short_numeric_hostname(host_str)
        || looks_all_numeric_labels(host_str)
    {
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
        port: effective_port,
    })
}

/// Default scheme port — 80 for http, 443 for https. Used by
/// `validate_url` to populate `SafeUrl::port` when the URL omits an
/// explicit port.
fn default_port_for_scheme(scheme: Scheme) -> u16 {
    match scheme {
        Scheme::Http => 80,
        Scheme::Https => 443,
    }
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

/// Detect short numeric forms with 2-3 dot-separated all-digit labels — e.g.
/// `127.1`, `1.2`, `1.2.3`. POSIX `inet_aton` and Windows `GetAddrInfoW`
/// expand these to full dotted-quads, so a hostile resolver could otherwise
/// land us on `127.0.0.1` despite our explicit numeric-hostname guard.
/// 4-part forms are handled by `looks_obfuscated_dotted_quad` /
/// `parse_strict_dotted_quad`. RFC 3696 §2 also forbids all-numeric TLDs,
/// so a final all-digit label is sufficient signal on its own for 2-3-part
/// hostnames.
fn looks_short_numeric_hostname(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if !(2..=3).contains(&parts.len()) {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Catches all-digit hostnames with 5+ labels (e.g. `127.0.0.0.1`) that
/// fall through both `looks_obfuscated_dotted_quad` (4 parts) and
/// `looks_short_numeric_hostname` (2-3 parts). Also catches any other
/// length where every label is purely numeric — RFC 3696 §2 forbids
/// all-numeric TLDs, so a final all-digit label combined with all-digit
/// preceding labels is never a legitimate hostname. Belt-and-suspenders
/// against POSIX/Windows resolvers expanding novel numeric forms.
fn looks_all_numeric_labels(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
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
    // 192.0.0.0/24 — IETF Protocol Assignments (incl. 192.0.0.170 NAT64
    // well-known). RFC 6890; never a legitimate fetch target.
    if o[0] == 192 && o[1] == 0 && o[2] == 0 {
        return true;
    }
    // 192.88.99.0/24 — deprecated 6to4 anycast (RFC 7526). Still occasionally
    // routed by misconfigured edge gear; deny defensively.
    if o[0] == 192 && o[1] == 88 && o[2] == 99 {
        return true;
    }
    // 198.18.0.0/15 — RFC 2544 benchmarking. Routinely live on internal lab
    // networks (Cisco inter-VRF benchmarking, F5 BIG-IP defaults).
    if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
        return true;
    }
    // RFC 5737 TEST-NET documentation ranges — must never appear on the
    // public Internet. A resolver that returns one of these for a public
    // hostname is misconfigured or hostile; treat as deny.
    //   192.0.2.0/24    (TEST-NET-1)
    //   198.51.100.0/24 (TEST-NET-2)
    //   203.0.113.0/24  (TEST-NET-3)
    if o[0] == 192 && o[1] == 0 && o[2] == 2 {
        return true;
    }
    if o[0] == 198 && o[1] == 51 && o[2] == 100 {
        return true;
    }
    if o[0] == 203 && o[1] == 0 && o[2] == 113 {
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
    // Deprecated site-local fec0::/10 (RFC 3879). Reserved and not
    // routable on the public Internet — treat like other non-global
    // scopes to avoid SSRF surprises.
    if (segs[0] & 0xffc0) == 0xfec0 {
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
    // ::a.b.c.d — deprecated IPv4-compatible IPv6. The unspecified (::)
    // and loopback (::1) addresses share this `[0;6]` prefix and are
    // already handled above; here we decode the embedded IPv4 and run
    // it through the v4 deny-list. This catches `::127.0.0.1`,
    // `::169.254.169.254`, etc. that `to_ipv4_mapped()` misses.
    if segs[0..6] == [0u16; 6] {
        let v4 = Ipv4Addr::new(
            (segs[6] >> 8) as u8,
            (segs[6] & 0xff) as u8,
            (segs[7] >> 8) as u8,
            (segs[7] & 0xff) as u8,
        );
        if is_denied_ipv4(v4) {
            return true;
        }
    }
    // 2002::/16 — 6to4. The next 32 bits encode an IPv4 address; if
    // that v4 is on the deny-list, the 6to4 wrapper must be too. Catches
    // 2002:7f00:0001::/48 (loopback) and 2002:a9fe:a9fe::/48 (AWS/GCP
    // metadata 169.254.169.254).
    if segs[0] == 0x2002 {
        let v4 = Ipv4Addr::new(
            (segs[1] >> 8) as u8,
            (segs[1] & 0xff) as u8,
            (segs[2] >> 8) as u8,
            (segs[2] & 0xff) as u8,
        );
        if is_denied_ipv4(v4) {
            return true;
        }
    }
    // 2001::/32 — Teredo (RFC 4380). The last 32 bits hold the client's
    // IPv4 address, obfuscated by bitwise-NOT. Decode and run through
    // the v4 deny-list so attackers can't smuggle loopback / metadata
    // IPs via a Teredo wrapper.
    if segs[0] == 0x2001 && segs[1] == 0x0000 {
        let v4 = Ipv4Addr::new(
            !((segs[6] >> 8) as u8),
            !((segs[6] & 0xff) as u8),
            !((segs[7] >> 8) as u8),
            !((segs[7] & 0xff) as u8),
        );
        if is_denied_ipv4(v4) {
            return true;
        }
    }
    false
}

/// Hostnames the deny-list must reject regardless of resolution. The
/// most common is GCP's metadata DNS name, which resolves to
/// 169.254.169.254 (already in the IP deny-list) — but a hostile
/// resolver could lie. CHAT.md
///
/// Covers loopback aliases (`localhost`, `ip6-localhost`, etc.) plus
/// known cloud-provider metadata DNS names. A trailing dot on input
/// (e.g. `metadata.google.internal.`) is normalized away before
/// matching, since FQDNs with and without the root dot are the same
/// name. Per RFC 6761, any hostname ending in `.localhost` is also
/// denied as loopback.
///
/// NOTE: Azure IMDS (169.254.169.254) has no DNS name; the public
/// `metadata.azure.com` ARM endpoint is intentionally NOT included.
pub fn is_denied_hostname(hostname: &str) -> bool {
    let lc = hostname.to_lowercase();
    // Strip a single trailing dot — `foo.` and `foo` denote the same
    // name in DNS and must produce the same deny decision.
    let normalized = lc.strip_suffix('.').unwrap_or(lc.as_str());
    if matches!(
        normalized,
        "metadata.google.internal"
            | "metadata"
            | "localhost"
            | "localhost.localdomain"
            | "ip6-localhost"
            | "ip6-loopback"
            | "instance-data"
            | "instance-data.ec2.internal"
            | "metadata.tencentyun.com"
            | "metadata.oracle.com"
            | "100-100.metadata.aliyuncs.com"
    ) {
        return true;
    }
    // RFC 6761: any name under the `.localhost` TLD is loopback.
    if normalized.ends_with(".localhost") {
        return true;
    }
    false
}

// ===== SSRF composition decision ===========================================

/// Decision returned by [`decide`] — `Allow` if every gate (URL form,
/// host deny-list, resolved-IP deny-list) passes, `Deny(reason)` with
/// a stable snake_case reason code otherwise. Used to factor the
/// fan-in of the URL+resolver checks out of `fetch` so it can be unit-
/// tested without spinning up a network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(&'static str),
}

/// Pure SSRF gate. Inputs:
/// - `safe` — the already-validated URL (so the URL-form gates have
///   already fired).
/// - `resolved` — every IP the resolver returned (or, for `Host::Ip`,
///   the literal address).
///
/// Returns `Allow` only if (a) the hostname (if any) isn't on the
/// `is_denied_hostname` list and (b) at least one resolved IP survives
/// `is_denied_ip` (the connect path is then pinned to the survivors).
/// On `Deny`, the reason code is one of the constants below — chosen
/// so each branch can be asserted in tests without relying on
/// stringly-typed prose.
pub fn decide(safe: &SafeUrl, resolved: &[IpAddr]) -> Decision {
    if let Host::Name(name) = &safe.host
        && is_denied_hostname(name)
    {
        return Decision::Deny("hostname_denied");
    }
    if resolved.is_empty() {
        return Decision::Deny("no_addrs");
    }
    if resolved.iter().copied().all(is_denied_ip) {
        return Decision::Deny("all_addrs_denied");
    }
    Decision::Allow
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
    use std::time::{Duration, Instant};
    // Single overall wall-clock budget for the entire fetch, including all
    // redirect hops and the per-hop DNS lookup. Without this, DNS could
    // burn FETCH_TIMEOUT_SECS and each of up to MAX_REDIRECTS+1 hops could
    // burn another FETCH_TIMEOUT_SECS, blocking the chat task for ~40s.
    let deadline = Instant::now() + Duration::from_secs(FETCH_TIMEOUT_SECS);
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let mut current = url.to_string();
    for hop in 0..=MAX_REDIRECTS {
        if remaining().is_zero() {
            return Err("fetch deadline exceeded".to_string());
        }
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
                // Bound DNS resolution by the same 5 s budget as the HTTP
                // call. Without this, a slow / hung system resolver could
                // block the chat task for the OS default (often 15 s+ on
                // Windows) before the reqwest timeout takes over.
                let addrs: Vec<SocketAddr> = match tokio::time::timeout(
                    remaining().min(Duration::from_secs(FETCH_TIMEOUT_SECS)),
                    tokio::net::lookup_host(lookup),
                )
                .await
                {
                    Ok(Ok(it)) => it.collect(),
                    Ok(Err(e)) => return Err(format!("DNS lookup failed: {e}")),
                    Err(_) => return Err("DNS lookup timed out".to_string()),
                };
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

        // Build a fresh reqwest client per call: we pin the resolver
        // to the validated IPs, which is a per-host config.
        let mut builder = reqwest::Client::builder()
            .timeout(remaining().min(Duration::from_secs(FETCH_TIMEOUT_SECS)))
            .connect_timeout(remaining().min(Duration::from_secs(2)))
            .redirect(reqwest::redirect::Policy::none())
            .https_only(false); // operator may explicitly fetch http URLs
        // Pin the full vetted IP set in ONE `resolve_to_addrs` call.
        // reqwest's `resolve_to_addrs` overwrites any prior override for
        // the same domain key (it's a HashMap insert), so calling it in a
        // loop with single-element slices leaves only the LAST accepted
        // IP pinned and lets the connect path fall back to system DNS for
        // anything else.
        //
        // For `Host::Ip` literals we ALSO call `resolve_to_addrs` keyed
        // on the literal IP string, defense-in-depth against any future
        // reqwest regression where a literal-IP connect honors a system
        // resolver or hosts-file override (some platforms — Windows
        // especially — can map literal IPs via custom name resolution
        // providers). The override key matches what reqwest extracts
        // from the URL's host component.
        let host_key = match &safe.host {
            Host::Name(n) => n.clone(),
            Host::Ip(IpAddr::V4(v4)) => v4.to_string(),
            Host::Ip(IpAddr::V6(v6)) => v6.to_string(),
        };
        builder = builder.resolve_to_addrs(&host_key, &resolved_addrs);
        let client = builder
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;

        // Single retry on transient transport failures for the FIRST hop
        // only. Retrying mid-redirect-chain is unsafe: a 3xx after one or
        // more redirects has already let the server see *something*, and
        // a redirect that landed on a now-failing connection might be
        // attacker-controlled (each `Location` is re-validated, but the
        // server's choice of where to point us is not).
        // `is_body()` failures mean the remote server already accepted
        // bytes — replaying would be a duplicate request, not a true retry.
        let send_result = client.get(&safe.url).send().await;
        let resp = match send_result {
            Ok(r) => r,
            Err(e) if hop == 0
                && (e.is_connect() || e.is_timeout())
                && !e.is_body()
                && remaining() >= std::time::Duration::from_secs(1) =>
            {
                tracing::debug!(
                    host = %match &safe.host {
                        Host::Name(n) => n.clone(),
                        Host::Ip(ip) => ip.to_string(),
                    },
                    err_kind = if e.is_timeout() { "timeout" } else { "connect" },
                    "[Chat] web_fetch first-hop transient transport error; retrying once"
                );
                client
                    .get(&safe.url)
                    .send()
                    .await
                    .map_err(|e| format!("fetch failed: {e}"))?
            }
            Err(e) => return Err(format!("fetch failed: {e}")),
        };

        // Operator-visible log of every hop (initial + each redirect).
        // Deliberately host+status only — never path/query, so secrets
        // embedded in URLs cannot leak via logs.
        let host_str = match &safe.host {
            Host::Name(n) => n.clone(),
            Host::Ip(ip) => ip.to_string(),
        };
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

        // Content-Type allowlist gate. Without this, an attacker URL serving
        // `application/octet-stream` or `image/*` would feed raw binary into
        // `String::from_utf8_lossy → strip_html_tags → LLM context` — a
        // wasted token budget at best, an indirect-prompt-injection / data
        // smuggling vector at worst. Missing Content-Type is allowed (older
        // servers commonly omit it; default to text-ish).
        if let Some(ct_hdr) = resp.headers().get(reqwest::header::CONTENT_TYPE) {
            let raw = ct_hdr.to_str().unwrap_or("");
            // Strip `; charset=…` (and any other parameters) and normalize.
            let ct = raw.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            let allowed = ct.starts_with("text/")
                || ct == "application/json"
                || ct == "application/xml"
                || ct == "application/xhtml+xml"
                || ct == "application/rss+xml"
                || ct == "application/atom+xml";
            if !allowed {
                return Err(format!("unsupported content-type '{ct}'"));
            }
        }

        // Content-Length pre-check: refuse oversize bodies before streaming a
        // single byte. Saves bandwidth and reduces slow-loris exposure. The
        // streaming `max_bytes` guard below is kept as defense-in-depth for
        // missing or lying Content-Length headers.
        if let Some(cl_hdr) = resp.headers().get(reqwest::header::CONTENT_LENGTH)
            && let Ok(cl_str) = cl_hdr.to_str()
            && let Ok(cl) = cl_str.trim().parse::<u64>()
            && cl > max_bytes as u64
        {
            return Err(format!(
                "response body Content-Length {cl} exceeds max_bytes {max_bytes}"
            ));
        }

        // Defend against decompression-bomb: refuse non-identity encodings.
        if let Some(enc) = resp.headers().get(reqwest::header::CONTENT_ENCODING) {
            let v = enc.to_str().unwrap_or("");
            if !v.is_empty() && !v.eq_ignore_ascii_case("identity") {
                return Err(format!("rejected Content-Encoding '{v}'"));
            }
        }
        // Streaming body read with running byte counter.
        //
        // The reqwest client carries a request-level timeout, but a
        // slow-loris peer dripping bytes *just under* that ceiling can
        // still pin the chat task for the full FETCH_TIMEOUT_SECS on
        // every hop. The `tokio::time::timeout` around each chunk-read
        // enforces the wall-clock budget BETWEEN chunks, so the deadline
        // is honored even when the request-level timer hasn't yet fired.
        //
        // Chunk-read error semantics: fail-closed. We drop any partial
        // body, emit a structured warn (host + partial_bytes + err_kind,
        // never path/query — secrets in URLs must not leak via logs),
        // and return the error to the caller. A truncated response would
        // be worse than no response — the LLM would happily reason over
        // an attacker-crafted prefix as if it were the full page.
        let mut body_bytes: Vec<u8> = Vec::with_capacity(8192);
        let mut total = 0usize;
        let mut stream = resp;
        loop {
            let chunk_res =
                match tokio::time::timeout(remaining(), stream.chunk()).await {
                    Ok(r) => r,
                    Err(_) => {
                        tracing::warn!(
                            host = %host_str,
                            partial_bytes = total,
                            "[Chat] web_fetch body read timed out"
                        );
                        return Err("body read deadline exceeded".to_string());
                    }
                };
            match chunk_res {
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
                Err(e) => {
                    // Fail-closed contract — clear the partial buffer and
                    // surface the error so the model sees the failure
                    // rather than a silently truncated page.
                    body_bytes.clear();
                    tracing::warn!(
                        host = %host_str,
                        partial_bytes = total,
                        err_kind = ?classify_chunk_err(&e),
                        "[Chat] web_fetch body chunk error"
                    );
                    return Err(format!("body chunk error: {e}"));
                }
            }
        }
        // CHAT.md: plain-text-only — strip HTML tags before returning.
        let text = String::from_utf8_lossy(&body_bytes).to_string();
        return Ok(strip_html_tags(&text));
    }
    Err("redirect loop terminated unexpectedly".to_string())
}

/// Coarse classifier for `reqwest::Error` to give the operator log a
/// stable, low-cardinality `err_kind` field rather than the full Display
/// (which can carry URLs, headers, and other content too noisy / risky
/// to drop into structured logs). Order matters — `is_timeout` is the
/// most-specific signal.
fn classify_chunk_err(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connect"
    } else if e.is_decode() {
        "decode"
    } else if e.is_body() {
        "body"
    } else {
        "other"
    }
}

fn port_for(safe: &SafeUrl) -> u16 {
    // Single source of truth — `validate_url` populates `safe.port` at
    // construction time so the connect path doesn't have to re-parse
    // the URL string. Kept as a function for callsite ergonomics and
    // to localize any future change (e.g. per-scheme overrides).
    safe.port
}

fn resolve_relative(base: &str, location: &str) -> Result<String, String> {
    // Absolute URL — pass through; will be re-validated. Match
    // case-insensitively because validate_url accepts mixed-case schemes.
    let loc_bytes = location.as_bytes();
    let loc_is_absolute = (loc_bytes.len() >= 7
        && loc_bytes[..7].eq_ignore_ascii_case(b"http://"))
        || (loc_bytes.len() >= 8 && loc_bytes[..8].eq_ignore_ascii_case(b"https://"));
    if loc_is_absolute {
        return Ok(location.to_string());
    }
    // Compute the base scheme; needed both for splicing and for resolving
    // protocol-relative redirects below.
    let base_bytes = base.as_bytes();
    let scheme_len = if base_bytes.len() >= 8
        && base_bytes[..8].eq_ignore_ascii_case(b"https://")
    {
        8
    } else if base_bytes.len() >= 7
        && base_bytes[..7].eq_ignore_ascii_case(b"http://")
    {
        7
    } else {
        return Err("base URL has no http(s) scheme".to_string());
    };
    let scheme = &base[..scheme_len];
    // Protocol-relative URL — `Location: //newhost/path`. The scheme is
    // inherited from the base, but the AUTHORITY changes, so the result
    // MUST be re-validated by `validate_url` against the deny-list. The
    // splice here just produces a syntactically absolute URL; the caller
    // (`fetch`) re-runs `validate_url` on every hop. Without this branch
    // a `//evil.example/` redirect would fall through to the path-relative
    // splice and produce a broken `https://victim/foo//evil.example/`,
    // silently masking the security implication that a new authority
    // appeared.
    if location.starts_with("//") {
        // `scheme` ends in exactly `//` here (the slice is `http://` or
        // `https://`). Use `strip_suffix("//")` rather than the greedy
        // `trim_end_matches('/')` — the latter would happily eat any
        // run of trailing slashes if the scheme were ever stored without
        // its terminating `//` (or with extras), masking the bug. The
        // exact suffix-strip is idempotent and asserts the shape we expect.
        let scheme_only = scheme.strip_suffix("//").unwrap_or(scheme);
        return Ok(format!("{scheme_only}{location}"));
    }
    // Path-absolute or path-relative — splice onto the base authority.
    let after = &base[scheme_len..];
    let path_start = after.find('/').unwrap_or(after.len());
    let authority = &after[..path_start];
    if let Some(stripped) = location.strip_prefix('/') {
        Ok(format!("{scheme}{authority}/{stripped}"))
    } else {
        // Path-relative — resolve against last `/` of the base path.
        let base_path = &after[path_start..];
        let prefix = if let Some(idx) = base_path.rfind('/') { &base_path[..=idx] } else { "/" };
        Ok(format!("{scheme}{authority}{prefix}{location}"))
    }
}

/// Tag names whose entire body (including the closing tag) is dropped.
/// JavaScript inside `<script>` and CSS inside `<style>` would otherwise
/// survive a naive `<…>` strip and reach the model as tool_result, where
/// it is a reliable indirect prompt-injection vector.
///
/// Stored pre-lowercased so `is_skip_block` can do an ASCII case-insensitive
/// compare via `eq_ignore_ascii_case` without re-lowering the table.
const SKIP_BLOCK_TAGS: &[&[u8]] = &[
    b"script",
    b"style",
    b"noscript",
    b"template",
    b"iframe",
    b"svg",
    b"math",
];

/// HTML tag stripper for `tool_result` inclusion. Operates as a small
/// state machine over the input bytes:
///
/// 1. Drops the *bodies* (and surrounding open/close tags) of script-like
///    elements (see [`SKIP_BLOCK_TAGS`]).
/// 2. Skips HTML comments correctly — a naive `<…>` toggle would
///    re-emit the rest of the document if a comment contained `>`.
/// 3. On unterminated tags / comments / skip-blocks, drops the tail
///    rather than re-emitting raw bytes (which would defeat the strip).
/// 4. AFTER tag stripping (order matters), decodes a small allowlist of
///    HTML entities. Doing it before would let
///    `&amp;lt;script&amp;gt;…` decode to `<script>…` and bypass step 1.
pub fn strip_html_tags(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'<' {
            // Not a tag start — push the next UTF-8 char (or this byte
            // verbatim if it's not at a char boundary; since we're
            // iterating a `&str`, splitting on '<' is safe).
            let ch_end = next_char_end(bytes, i);
            out.push_str(&input[i..ch_end]);
            i = ch_end;
            continue;
        }
        // Comment: `<!-- … -->`. Drop entirely; if unterminated, drop tail.
        if bytes[i..].starts_with(b"<!--") {
            match find_subslice(&bytes[i + 4..], b"-->") {
                Some(rel) => i += 4 + rel + 3,
                None => return finalize_strip(&out),
            }
            continue;
        }
        // CDATA: `<![CDATA[ … ]]>`. Must be matched BEFORE the generic
        // `<!` markup-declaration branch below, which only scans for the
        // next `>` byte and would terminate at the first `>` inside the
        // CDATA body — re-emitting attacker text (e.g. the contents of a
        // smuggled `<script>` opener) into the tool_result. The
        // Content-Type allow-list accepts XML/XHTML/RSS/Atom, where CDATA
        // is legal markup, so this branch is reachable in production.
        if bytes[i..].starts_with(b"<![CDATA[") {
            match find_subslice(&bytes[i + 9..], b"]]>") {
                Some(rel) => i += 9 + rel + 3,
                None => return finalize_strip(&out),
            }
            continue;
        }
        // Other markup declarations (`<!DOCTYPE …>`) and processing
        // instructions (`<? … ?>`): drop up to the next `>`, or the
        // tail if unterminated.
        if bytes[i..].starts_with(b"<!") || bytes[i..].starts_with(b"<?") {
            match memchr(b'>', &bytes[i..]) {
                Some(rel) => i += rel + 1,
                None => return finalize_strip(&out),
            }
            continue;
        }
        // Regular tag: `<name…>` or `</name…>`. Extract the tag name
        // (terminated by whitespace, `>`, or `/`) and check whether
        // it's a skip-block opener.
        let after_lt = i + 1;
        let is_close = after_lt < bytes.len() && bytes[after_lt] == b'/';
        let name_start = if is_close { after_lt + 1 } else { after_lt };
        let mut name_end = name_start;
        while name_end < bytes.len() {
            let nb = bytes[name_end];
            if nb == b'>' || nb == b'/' || (nb as char).is_ascii_whitespace() {
                break;
            }
            name_end += 1;
        }
        // Find end of this tag (`>`); on unterminated tag, drop tail.
        let tag_end = match memchr(b'>', &bytes[name_end..]) {
            Some(rel) => name_end + rel + 1,
            None => return finalize_strip(&out),
        };
        let name = &bytes[name_start..name_end];
        if !is_close && is_skip_block(name) {
            // Scan forward to a matching `</name>` (case-insensitive,
            // optional whitespace and `>` after the name).
            let body_start = tag_end;
            match find_close_tag(bytes, body_start, name) {
                Some(close_end) => {
                    // Insert a space so adjacent text on either side of
                    // the dropped block doesn't get glued together
                    // (e.g. `middle<style>…</style>after`).
                    out.push(' ');
                    i = close_end;
                }
                // Unterminated skip-block — drop everything from here on.
                None => return finalize_strip(&out),
            }
            continue;
        }
        // Ordinary tag — drop it.
        i = tag_end;
    }
    finalize_strip(&out)
}

/// Whitespace-collapse + trim + entity-decode. Entity decode runs LAST
/// so that any encoded markup in the document is rendered as literal
/// text instead of being re-fed to the stripper.
fn finalize_strip(stripped: &str) -> String {
    // Collapse whitespace runs.
    let mut collapsed = String::with_capacity(stripped.len());
    let mut prev_ws = false;
    for c in stripped.chars() {
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
    let trimmed = collapsed.trim();
    decode_entities(trimmed)
}

fn is_skip_block(name: &[u8]) -> bool {
    SKIP_BLOCK_TAGS
        .iter()
        .any(|t| t.eq_ignore_ascii_case(name))
}

/// Find a closing `</name>` starting at `from`, case-insensitive, where
/// the close tag is `</name`, optional ASCII whitespace, then `>`. On a
/// name mismatch the scan advances to the NEXT `<` rather than the next
/// byte — turning a quadratic O(n*m) scan into a linear pass through
/// `memchr(b'<', …)`. This matters on adversarial input where the body
/// is mostly `<` bytes that don't open `</name>`.
fn find_close_tag(bytes: &[u8], from: usize, name: &[u8]) -> Option<usize> {
    let mut i = from;
    while i + 2 < bytes.len() {
        // Jump straight to the next `<`. If none remains, no close tag.
        match memchr(b'<', &bytes[i..]) {
            Some(rel) => i += rel,
            None => return None,
        }
        if i + 2 >= bytes.len() {
            return None;
        }
        if bytes[i + 1] != b'/' {
            // Not a close-tag opener — advance past this `<` and keep
            // scanning. Crucially one step past the `<` so the next
            // `memchr` doesn't re-find the same byte.
            i += 1;
            continue;
        }
        let n_start = i + 2;
        let n_end = n_start + name.len();
        if n_end > bytes.len() {
            return None;
        }
        if !bytes[n_start..n_end].eq_ignore_ascii_case(name) {
            i += 1;
            continue;
        }
        let mut j = n_end;
        while j < bytes.len() && (bytes[j] as char).is_ascii_whitespace() {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'>' {
            return Some(j + 1);
        }
        i += 1;
    }
    None
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

/// Step `i` forward past one UTF-8 char in `bytes` (which MUST be the
/// bytes of a `&str`, so `i` is always at a char boundary).
fn next_char_end(bytes: &[u8], i: usize) -> usize {
    let b = bytes[i];
    let len = if b < 0x80 {
        1
    } else if b < 0xc0 {
        // Continuation byte — shouldn't happen at a boundary, but be safe.
        1
    } else if b < 0xe0 {
        2
    } else if b < 0xf0 {
        3
    } else {
        4
    };
    (i + len).min(bytes.len())
}

/// Decode a small allowlist of HTML entities. Anything not on the list
/// (including malformed numeric escapes) is passed through verbatim.
fn decode_entities(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            let end = next_char_end(bytes, i);
            out.push_str(&input[i..end]);
            i = end;
            continue;
        }
        // Find the terminating `;` within a small window — entities are
        // short; cap at 10 chars beyond the `&` to avoid scanning the
        // entire document for a stray ampersand.
        let cap = (i + 12).min(bytes.len());
        let semi = bytes[i + 1..cap].iter().position(|&b| b == b';');
        let Some(rel) = semi else {
            out.push('&');
            i += 1;
            continue;
        };
        let body = &input[i + 1..i + 1 + rel];
        let decoded: Option<char> = match body {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{00A0}'),
            _ => decode_numeric_entity(body),
        };
        if let Some(c) = decoded {
            out.push(c);
            i += 1 + rel + 1; // past `&body;`
        } else {
            out.push('&');
            i += 1;
        }
    }
    out
}

fn decode_numeric_entity(body: &str) -> Option<char> {
    let rest = body.strip_prefix('#')?;
    let cp: u32 = if let Some(hex) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X')) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()?
    } else {
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        rest.parse().ok()?
    };
    if cp > 0x10_FFFF || (0xD800..=0xDFFF).contains(&cp) {
        return None;
    }
    // Refuse to decode ASCII control chars — `&#10;` (LF), `&#13;` (CR),
    // `&#0;` (NUL), `&#127;` (DEL), etc. Smuggling CR/LF/NUL into a
    // tool_result text via a crafted page would otherwise let an attacker
    // forge fresh chat lines / break log framing downstream. Pass the
    // entity through verbatim so it appears as literal `&#10;` text and
    // the sanitize_outbound_chat boundary remains the single source of
    // truth for what control bytes can reach the chat sink.
    if cp < 0x20 || cp == 0x7F {
        return None;
    }
    char::from_u32(cp)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- validate_url ---------------------------------------------------

    #[test]
    fn uppercase_scheme_accepted_and_resolves_relative() {
        // Regression: validate_url used to lowercase the entire URL and
        // slice the original by lowercase-derived length, which panicked
        // when Unicode chars changed byte length on lowercase. Verify the
        // uppercase case still works.
        let u = validate_url("HTTPS://example.com/path").unwrap();
        assert_eq!(u.scheme, Scheme::Https);
        // resolve_relative used case-sensitive `starts_with` so a
        // mixed-case base URL would error. Verify it now works.
        let r = resolve_relative("HTTPS://example.com/a/b", "/c").unwrap();
        assert_eq!(r, "HTTPS://example.com/c");
    }

    #[test]
    fn https_with_textual_host_accepted() {
        let u = validate_url("https://example.com/path").unwrap();
        assert_eq!(u.scheme, Scheme::Https);
        assert!(matches!(u.host, Host::Name(ref n) if n == "example.com"));
    }

    #[test]
    fn http_with_port_accepted() {
        // Port 80 is on the allow-list.
        let u = validate_url("http://example.com:80/").unwrap();
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
    fn accepts_query_only_after_authority() {
        // Regression: authority extraction used to split only on `/`,
        // which meant `http://example.com?x=1` produced
        // authority = "example.com?x=1" and was rejected as BadHost.
        let u = validate_url("http://example.com?x=1").unwrap();
        assert!(matches!(u.host, Host::Name(ref n) if n == "example.com"));
    }

    #[test]
    fn accepts_fragment_only_after_authority() {
        let u = validate_url("http://example.com#frag").unwrap();
        assert!(matches!(u.host, Host::Name(ref n) if n == "example.com"));
    }

    #[test]
    fn port_preserved_with_query_only() {
        // The port must still be stripped correctly when the authority
        // is followed by `?` rather than `/`. Use 443 (on the allow-list).
        let u = validate_url("http://example.com:443?x=1").unwrap();
        assert!(matches!(u.host, Host::Name(ref n) if n == "example.com"));
    }

    #[test]
    fn rejects_userinfo_with_query_only() {
        // Userinfo rejection still fires when authority is terminated
        // by `?` instead of `/`.
        assert_eq!(
            validate_url("http://user@example.com?x=1").unwrap_err(),
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
    fn rejects_short_numeric_hostnames() {
        // POSIX inet_aton and Windows GetAddrInfoW expand these to full
        // dotted-quads (e.g. `127.1` → 127.0.0.1), so the dual hostname/IP
        // gate must reject them at the parser layer rather than relying on
        // the resolver-side IP deny-list.
        for s in [
            "http://127.1/",
            "http://0.1/",
            "http://1.2/",
            "http://1.2.3/",
        ] {
            assert_eq!(
                validate_url(s).unwrap_err(),
                UrlError::NumericHostname,
                "should be rejected: {s}"
            );
        }
        // Mixed digit/text labels with a non-numeric final label (legitimate
        // hostnames per RFC 3696 §2) must still validate.
        assert!(validate_url("http://1.example.com/").is_ok());
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

    // ---- port allow-list ------------------------------------------------

    #[test]
    fn rejects_disallowed_port_smtp() {
        // 25 — SMTP. Public host with internal-protocol port must be
        // rejected even though the IP itself would be allowed.
        assert_eq!(
            validate_url("http://example.com:25/").unwrap_err(),
            UrlError::DisallowedPort(25)
        );
    }

    #[test]
    fn rejects_disallowed_port_redis() {
        // 6379 — Redis. The headline case: `victim.example.com:6379`.
        assert_eq!(
            validate_url("http://example.com:6379/").unwrap_err(),
            UrlError::DisallowedPort(6379)
        );
    }

    #[test]
    fn rejects_disallowed_port_ollama() {
        // 11434 — Ollama default. Local LLM exfil vector.
        assert_eq!(
            validate_url("http://example.com:11434/").unwrap_err(),
            UrlError::DisallowedPort(11434)
        );
    }

    #[test]
    fn implicit_port_allowed_baseline() {
        // No explicit port → port_for substitutes 80/443. Must validate.
        validate_url("http://example.com/").unwrap();
        validate_url("https://example.com/").unwrap();
    }

    #[test]
    fn explicit_port_80_and_443_allowed() {
        validate_url("http://example.com:80/").unwrap();
        validate_url("https://example.com:443/").unwrap();
        // Cross-pairings (https on :80, http on :443) are still allowed
        // by the port gate — the deny semantic is "port not in list",
        // not "port matches scheme default".
        validate_url("https://example.com:80/").unwrap();
        validate_url("http://example.com:443/").unwrap();
    }

    #[test]
    fn ipv6_literal_disallowed_port_rejected() {
        // IPv6 path has its own port-parse branch — must enforce too.
        assert_eq!(
            validate_url("http://[2001:db8::1]:6379/").unwrap_err(),
            UrlError::DisallowedPort(6379)
        );
        assert_eq!(
            validate_url("http://[2001:db8::1]:25/").unwrap_err(),
            UrlError::DisallowedPort(25)
        );
        assert_eq!(
            validate_url("http://[2001:db8::1]:11434/").unwrap_err(),
            UrlError::DisallowedPort(11434)
        );
    }

    #[test]
    fn ipv6_literal_allowed_ports_accepted() {
        // Implicit port (no `:port` after `]`).
        validate_url("http://[2001:db8::1]/").unwrap();
        // Explicit :80 / :443.
        validate_url("http://[2001:db8::1]:80/").unwrap();
        validate_url("https://[2001:db8::1]:443/").unwrap();
    }

    #[test]
    fn ipv4_literal_disallowed_port_rejected() {
        assert_eq!(
            validate_url("http://1.2.3.4:6379/").unwrap_err(),
            UrlError::DisallowedPort(6379)
        );
    }

    #[test]
    fn disallowed_port_display_is_operator_readable() {
        // The Display impl surfaces a human-friendly message including
        // the offending port. Used by `fetch`'s error formatting.
        let msg = format!("{}", UrlError::DisallowedPort(6379));
        assert!(msg.contains("6379"), "got {msg:?}");
        assert!(msg.contains("allow"), "got {msg:?}");
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
    fn ipv4_compatible_v6_consults_v4_denylist() {
        // ::a.b.c.d — deprecated IPv4-compatible form. `to_ipv4_mapped()`
        // does NOT match these, so without the explicit prefix check
        // attackers could smuggle loopback / cloud-metadata IPs through.
        assert!(is_denied_ip("::127.0.0.1".parse().unwrap()));
        assert!(is_denied_ip("::169.254.169.254".parse().unwrap()));
        // Public address embedded in the same prefix should still be
        // allowed — the wrapper itself is not a deny criterion.
        assert!(!is_denied_ip("::8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn ietf_protocol_assignments_denied() {
        // 192.0.0.0/24 — RFC 6890 IETF Protocol Assignments. Includes the
        // 192.0.0.170 NAT64 well-known prefix which a hostile resolver
        // could otherwise return as a public-looking address.
        assert!(is_denied_ip("192.0.0.0".parse().unwrap()));
        assert!(is_denied_ip("192.0.0.170".parse().unwrap()));
        assert!(is_denied_ip("192.0.0.255".parse().unwrap()));
        // Adjacent /24 outside the range should still be allowed.
        assert!(!is_denied_ip("192.0.1.1".parse().unwrap()));
    }

    #[test]
    fn deprecated_6to4_anycast_denied() {
        // 192.88.99.0/24 — RFC 7526 deprecated 6to4 anycast.
        assert!(is_denied_ip("192.88.99.1".parse().unwrap()));
        assert!(is_denied_ip("192.88.99.254".parse().unwrap()));
        assert!(!is_denied_ip("192.88.100.1".parse().unwrap()));
    }

    #[test]
    fn benchmarking_range_denied() {
        // 198.18.0.0/15 — RFC 2544. Cisco inter-VRF benchmarking and F5
        // BIG-IP defaults route this internally; a hostile resolver could
        // land on it to reach lab/internal infrastructure.
        assert!(is_denied_ip("198.18.0.0".parse().unwrap()));
        assert!(is_denied_ip("198.18.0.1".parse().unwrap()));
        assert!(is_denied_ip("198.19.255.254".parse().unwrap()));
        // Just outside the /15 should still be allowed.
        assert!(!is_denied_ip("198.17.255.254".parse().unwrap()));
        assert!(!is_denied_ip("198.20.0.1".parse().unwrap()));
    }

    #[test]
    fn six_to_four_v6_consults_v4_denylist() {
        // 2002:7f00:0001:: — 6to4-wrapped 127.0.0.1.
        assert!(is_denied_ip("2002:7f00:0001::".parse().unwrap()));
        // 2002:a9fe:a9fe:: — 6to4-wrapped 169.254.169.254 (AWS/GCP metadata).
        assert!(is_denied_ip("2002:a9fe:a9fe::".parse().unwrap()));
        // 2002:0808:0808:: — 6to4-wrapped 8.8.8.8, public, allowed.
        assert!(!is_denied_ip("2002:0808:0808::".parse().unwrap()));
    }

    #[test]
    fn teredo_v6_consults_v4_denylist() {
        // 2001:0:102:304:0:0:f5ff:fffe — Teredo with embedded client v4
        // ~0xf5fffffe = 10.0.0.1 (private RFC 1918).
        assert!(is_denied_ip("2001:0:102:304:0:0:f5ff:fffe".parse().unwrap()));
        // 2001:0:102:304:0:0:f7f7:f7f7 — Teredo with embedded client v4
        // ~0xf7f7f7f7 = 8.8.8.8, public, allowed (no false positive on
        // every 2001::/32 address).
        assert!(!is_denied_ip("2001:0:102:304:0:0:f7f7:f7f7".parse().unwrap()));
    }

    #[test]
    fn site_local_v6_denied() {
        // fec0::/10 is deprecated (RFC 3879) but must still be rejected
        // since it's non-global scope.
        assert!(is_denied_ip("fec0::1".parse().unwrap()));
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
        // A name that merely contains "localhost" as a substring must
        // not be denied — only the exact label or a `.localhost` suffix.
        assert!(!is_denied_hostname("notlocalhost.com"));
    }

    #[test]
    fn loopback_aliases_denied() {
        assert!(is_denied_hostname("localhost"));
        assert!(is_denied_hostname("LOCALHOST"));
        assert!(is_denied_hostname("localhost.localdomain"));
        assert!(is_denied_hostname("ip6-localhost"));
        assert!(is_denied_hostname("ip6-loopback"));
    }

    #[test]
    fn cloud_metadata_hostnames_denied() {
        // AWS — legacy bare label and EC2 internal FQDN.
        assert!(is_denied_hostname("instance-data"));
        assert!(is_denied_hostname("instance-data.ec2.internal"));
        // Tencent Cloud, Oracle Cloud, Alibaba Cloud.
        assert!(is_denied_hostname("metadata.tencentyun.com"));
        assert!(is_denied_hostname("metadata.oracle.com"));
        assert!(is_denied_hostname("100-100.metadata.aliyuncs.com"));
    }

    #[test]
    fn dot_localhost_suffix_denied() {
        // RFC 6761: anything under .localhost is loopback.
        assert!(is_denied_hostname("foo.localhost"));
        assert!(is_denied_hostname("a.b.c.localhost"));
    }

    #[test]
    fn trailing_dot_normalized() {
        // FQDN trailing-dot form must match the same deny decision.
        assert!(is_denied_hostname("localhost."));
        assert!(is_denied_hostname("metadata.google.internal."));
        assert!(!is_denied_hostname("example.com."));
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
    fn strip_html_tags_drops_script_body() {
        let s = strip_html_tags("<script>alert(1)</script>");
        assert!(s.is_empty() || s.chars().all(char::is_whitespace), "got {s:?}");
    }

    #[test]
    fn strip_html_tags_script_case_insensitive() {
        let s = strip_html_tags("<SCRIPT>x</SCRIPT>");
        assert!(s.is_empty() || s.chars().all(char::is_whitespace), "got {s:?}");
    }

    #[test]
    fn strip_html_tags_drops_script_with_attrs_and_style() {
        let html = "<script src=x>js</script>middle<style>.x{}</style>after";
        assert_eq!(strip_html_tags(html), "middle after");
    }

    #[test]
    fn strip_html_tags_decodes_after_strip_not_before() {
        // Encoded markup must render as literal text, NOT be re-stripped.
        // Order matters: if the decode ran first, the stripper would eat
        // the inner script tags and we'd lose the alert(1) text.
        let html = "&lt;script&gt;alert(1)&lt;/script&gt;";
        assert_eq!(strip_html_tags(html), "<script>alert(1)</script>");
    }

    #[test]
    fn strip_html_tags_drops_comments_with_inner_gt() {
        // A naive `<…>` toggle would re-emit `x</script> --&gt;` here
        // because of the `>` inside the comment. The state machine
        // recognises `<!--` and scans for `-->`, so the whole thing dies.
        let html = "<!-- <script>x</script> --&gt;";
        let s = strip_html_tags(html);
        assert!(s.is_empty() || s.chars().all(char::is_whitespace), "got {s:?}");
    }

    #[test]
    fn strip_html_tags_unterminated_script_drops_tail() {
        let s = strip_html_tags("<script>foo");
        assert!(s.is_empty(), "got {s:?}");
    }

    #[test]
    fn strip_html_tags_drops_cdata_block() {
        // The Content-Type allow-list accepts XML/XHTML/RSS/Atom, where
        // `<![CDATA[ … ]]>` is legal markup. The generic `<!` branch
        // would terminate at the first `>` inside the CDATA body —
        // re-emitting attacker `alert(1)` text into tool_result.
        let s = strip_html_tags("<![CDATA[<script>alert(1)</script>]]>");
        assert!(!s.contains("alert(1)"), "got {s:?}");
    }

    #[test]
    fn strip_html_tags_drops_unterminated_cdata() {
        // No `]]>` and no `</script>` either — the unterminated-CDATA
        // path must drop the tail rather than re-emit raw attacker text.
        let s = strip_html_tags("<![CDATA[<script>alert(1)");
        assert!(!s.contains("alert(1)"), "got {s:?}");
    }

    #[test]
    fn strip_html_tags_decodes_allowed_entities() {
        assert_eq!(strip_html_tags("a &amp; b"), "a & b");
        assert_eq!(strip_html_tags("&quot;x&quot;"), "\"x\"");
        assert_eq!(strip_html_tags("&apos;y&apos;"), "'y'");
        assert_eq!(strip_html_tags("&#39;z&#39;"), "'z'");
        assert_eq!(strip_html_tags("&#x41;"), "A");
    }

    #[test]
    fn strip_html_tags_passes_malformed_entities_through() {
        // No semicolon, surrogate, out-of-range, unknown name — verbatim.
        assert_eq!(strip_html_tags("AT&T"), "AT&T");
        assert_eq!(strip_html_tags("&#xD800;"), "&#xD800;");
        assert_eq!(strip_html_tags("&#x110000;"), "&#x110000;");
        assert_eq!(strip_html_tags("&unknown;"), "&unknown;");
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
    fn resolve_relative_path_relative_against_pathless_base() {
        // Regression: a 3xx response with `Location: foo` from a path-less
        // base URL used to panic — `base_path = ""`, `rfind('/').unwrap_or(0)`
        // gave 0, then `&base_path[..=0]` sliced `0..1` on an empty string.
        assert_eq!(
            resolve_relative("http://example.com", "foo").unwrap(),
            "http://example.com/foo"
        );
        // Same regression class with a query-only relative target.
        assert_eq!(
            resolve_relative("http://example.com", "?q=1").unwrap(),
            "http://example.com/?q=1"
        );
    }

    #[test]
    fn resolve_relative_protocol_relative_inherits_scheme() {
        // `//evil.com/path` against an https base must yield
        // `https://evil.com/path` — NOT a path-relative splice that
        // glues the second authority onto the base's path. The result
        // is then re-fed to `validate_url` by the fetch loop, which is
        // the actual security gate.
        let r = resolve_relative("https://example.com/foo", "//evil.com/path").unwrap();
        assert_eq!(r, "https://evil.com/path");
        // And the resolved URL must validate cleanly as a normal https
        // URL — i.e. the new authority is what `validate_url` sees.
        let safe = validate_url(&r).unwrap();
        assert_eq!(safe.scheme, Scheme::Https);
        assert!(matches!(safe.host, Host::Name(ref n) if n == "evil.com"));

        // http base, protocol-relative redirect — scheme inherits.
        let r = resolve_relative("http://example.com/", "//other.com/").unwrap();
        assert_eq!(r, "http://other.com/");
    }

    #[test]
    fn port_for_uses_default_when_unspecified() {
        let safe = validate_url("https://example.com/x").unwrap();
        assert_eq!(port_for(&safe), 443);
        assert_eq!(safe.port, 443);
        let safe = validate_url("http://example.com/").unwrap();
        assert_eq!(port_for(&safe), 80);
        assert_eq!(safe.port, 80);
    }

    #[test]
    fn port_for_extracts_explicit_port() {
        // Only allow-listed ports survive validate_url; 80/443 stand in
        // for the explicit-port path. The stored `port` field must reflect
        // whatever the URL specified.
        let safe = validate_url("http://example.com:80/path").unwrap();
        assert_eq!(port_for(&safe), 80);
        assert_eq!(safe.port, 80);
        let safe = validate_url("https://example.com:443/path").unwrap();
        assert_eq!(safe.port, 443);
    }

    #[test]
    fn port_for_extracts_explicit_port_when_authority_terminated_by_query() {
        // Regression: authority can end at `?` (no path). The validator's
        // port-extraction must split on `?`/`#`/`/` so an authority like
        // `example.com:443?x=1` parses cleanly and `SafeUrl::port` reflects
        // the explicit value rather than silently falling back to the
        // scheme default.
        let safe = validate_url("http://example.com:443?x=1").unwrap();
        assert_eq!(port_for(&safe), 443);
        assert_eq!(safe.port, 443);
    }

    #[test]
    fn port_for_extracts_explicit_port_when_authority_terminated_by_fragment() {
        // Regression: authority can end at `#` (no path).
        let safe = validate_url("http://example.com:443#frag").unwrap();
        assert_eq!(port_for(&safe), 443);
        assert_eq!(safe.port, 443);
    }

    #[test]
    fn port_for_extracts_explicit_port_ipv6_when_authority_terminated_by_query() {
        // Regression: same bug shape for IPv6 authority — `after_close`
        // would otherwise carry `:443?x=1` and `strip_prefix(':').parse()`
        // would fail, masking the explicit port.
        let safe = validate_url("http://[2001:db8::1]:443?x=1").unwrap();
        assert_eq!(port_for(&safe), 443);
        assert_eq!(safe.port, 443);
    }

    // ---- SSRF composition: decide() ------------------------------------
    //
    // The decision function is the pure fan-in of the URL / host /
    // resolved-IP gates that `fetch` consults at runtime. Testing it
    // directly lets us pin EVERY deny path without a network — the
    // fetch glue would otherwise need fixtures for DNS, redirects, and
    // a real socket layer.

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn decide_allows_public_host_resolving_to_public_ip() {
        let safe = validate_url("https://example.com/").unwrap();
        assert_eq!(decide(&safe, &[ip("8.8.8.8")]), Decision::Allow);
    }

    #[test]
    fn decide_denies_literal_loopback_ip() {
        // Literal-IP URL — the IP itself is on the deny-list.
        let safe = SafeUrl {
            scheme: Scheme::Http,
            host: Host::Ip(ip("127.0.0.1")),
            url: "http://127.0.0.1/".to_string(),
            port: 80,
        };
        assert_eq!(
            decide(&safe, &[ip("127.0.0.1")]),
            Decision::Deny("all_addrs_denied")
        );
    }

    #[test]
    fn decide_denies_literal_metadata_ip() {
        let safe = SafeUrl {
            scheme: Scheme::Http,
            host: Host::Ip(ip("169.254.169.254")),
            url: "http://169.254.169.254/".to_string(),
            port: 80,
        };
        assert_eq!(
            decide(&safe, &[ip("169.254.169.254")]),
            Decision::Deny("all_addrs_denied")
        );
    }

    #[test]
    fn decide_denies_hostname_resolving_to_loopback() {
        // DNS rebinding shape: public-looking host, resolver returns
        // a loopback IP. The decision must trip on the resolved IP set.
        let safe = validate_url("https://attacker.example/").unwrap();
        assert_eq!(
            decide(&safe, &[ip("127.0.0.1")]),
            Decision::Deny("all_addrs_denied")
        );
    }

    #[test]
    fn decide_denies_hostname_resolving_to_metadata() {
        // Same shape, AWS/GCP metadata IP — the classic SSRF target.
        let safe = validate_url("https://attacker.example/").unwrap();
        assert_eq!(
            decide(&safe, &[ip("169.254.169.254")]),
            Decision::Deny("all_addrs_denied")
        );
    }

    #[test]
    fn decide_denies_hostname_literal_localhost() {
        // The hostname `localhost` itself is on the host deny-list and
        // must fire BEFORE the IP gate so the operator log gets the
        // right reason code (resolver-independent decision).
        let safe = validate_url("https://localhost/").unwrap();
        // Even with a public resolved IP, the hostname gate wins.
        assert_eq!(
            decide(&safe, &[ip("8.8.8.8")]),
            Decision::Deny("hostname_denied")
        );
    }

    #[test]
    fn decide_redirect_to_metadata_is_denied() {
        // A 302 → http://169.254.169.254/… is re-validated through
        // `decide` on the redirect hop. The new URL parses as a literal
        // metadata IP and the deny-list catches it.
        let safe = validate_url("http://169.254.169.254/latest/meta-data").unwrap();
        assert_eq!(
            decide(&safe, &[ip("169.254.169.254")]),
            Decision::Deny("all_addrs_denied")
        );
    }

    #[test]
    fn decide_denies_test_net_ranges() {
        // RFC 5737 — documentation ranges that must never reach a real
        // network. A resolver returning one of these is misconfigured /
        // hostile and the connect path must refuse it.
        for s in ["192.0.2.1", "198.51.100.42", "203.0.113.7"] {
            let safe = validate_url("https://docs.example/").unwrap();
            assert_eq!(
                decide(&safe, &[ip(s)]),
                Decision::Deny("all_addrs_denied"),
                "TEST-NET addr {s} should be denied"
            );
        }
    }

    #[test]
    fn decide_allows_when_at_least_one_resolved_ip_is_public() {
        // Mixed resolver answer — pin to the public survivor. (`fetch`
        // builds the pinned-IP set from `is_denied_ip`-filtered survivors;
        // `decide`'s job is only to say "at least one is OK".)
        let safe = validate_url("https://mixed.example/").unwrap();
        assert_eq!(
            decide(&safe, &[ip("10.0.0.1"), ip("8.8.8.8")]),
            Decision::Allow
        );
    }

    #[test]
    fn decide_denies_empty_resolution() {
        // Resolver returned no addresses — there's nothing to connect to.
        let safe = validate_url("https://example.com/").unwrap();
        assert_eq!(decide(&safe, &[]), Decision::Deny("no_addrs"));
    }

    // ---- length / format gate ------------------------------------------

    #[test]
    fn rejects_url_exceeding_length_cap() {
        // Defends against DoS-on-cheap-input from prompt-injection-driven
        // huge URL construction.
        let host = "x".repeat(10_000);
        let url = format!("http://{host}/");
        assert_eq!(validate_url(&url).unwrap_err(), UrlError::BadFormat);
    }

    #[test]
    fn rejects_zero_width_format_chars_in_authority() {
        // Cf format chars (U+200B etc.) are invisible in editor/log
        // output. The URL gate must refuse them up front so an injected
        // ZWSP-bearing URL doesn't sneak past a casual reviewer.
        for c in [
            '\u{200B}', '\u{200E}', '\u{200F}',
            '\u{2028}', '\u{2029}', '\u{FEFF}',
        ] {
            let url = format!("http://example{c}.com/");
            assert_eq!(
                validate_url(&url).unwrap_err(),
                UrlError::BadFormat,
                "Cf char {c:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_five_part_all_digit_hostname() {
        // `looks_obfuscated_dotted_quad` only fires on 4-part forms and
        // `looks_short_numeric_hostname` only on 2-3-part. A 5+-part
        // all-digit hostname (e.g. `127.0.0.0.1`) must still be caught.
        assert_eq!(
            validate_url("http://127.0.0.0.1/").unwrap_err(),
            UrlError::NumericHostname
        );
        assert_eq!(
            validate_url("http://1.2.3.4.5.6/").unwrap_err(),
            UrlError::NumericHostname
        );
    }

    // ---- entity decode security ----------------------------------------

    #[test]
    fn numeric_entity_control_chars_passed_through_verbatim() {
        // CR/LF/NUL/DEL via numeric entity is a chat-command-injection
        // smuggling shape — decoding it would let an attacker page
        // forge `\n/op foo` into the tool_result text. The decoder must
        // refuse and emit the entity literally.
        for body in ["&#10;", "&#13;", "&#0;", "&#127;", "&#x0A;", "&#x7F;"] {
            assert_eq!(
                strip_html_tags(body),
                body,
                "control-char numeric entity {body:?} should not decode"
            );
        }
        // Sanity: printable codepoints still decode.
        assert_eq!(strip_html_tags("&#65;"), "A");
        assert_eq!(strip_html_tags("&#x41;"), "A");
    }

    // ---- resolve_relative protocol-relative idempotence ----------------

    #[test]
    fn resolve_relative_protocol_relative_idempotent_strip() {
        // Defensive: even if the scheme slice somehow loses its trailing
        // `//` (it doesn't today, but a refactor could), the result
        // should still produce a syntactically valid URL — the new
        // `strip_suffix("//")` is idempotent on mis-shaped input.
        let r = resolve_relative("https://example.com/x", "//other.com/").unwrap();
        assert_eq!(r, "https://other.com/");
    }
}
