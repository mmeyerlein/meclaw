//! GH #833: whose identity header a mount believes.
//!
//! Two cell types read an identity header on a handed connection: the peer
//! mount of `proxy/meclaw` (`identity_header`, the sender of every frame) and
//! `web` (`hop.user_id`). A header is only as good as the hop that wrote it:
//! a reverse proxy in front authenticates and writes it, and a client that
//! reaches the listener directly can write the same line. The listener knows
//! which of the two it is talking to — the peer address of the accepted
//! connection travels in [`super::HandedConnection`] — so the mount asks this
//! module once per connection whether that address is a proxy it trusts.
//!
//! `std::net` only, on purpose: the tech stack carries no IP-prefix crate
//! (`ipnet` sits in the lock file only transitively), and a membership test
//! over a mask is twenty lines. The v4-mapped normalisation is the one of
//! `meclaw_cells::web_fetch` for the one form a listener bound to `[::]`
//! actually reports (`::ffff:a.b.c.d`); the NAT64 and 6to4 forms that module
//! also unfolds are NOT unfolded here — a remote 6to4 address that embeds
//! `127.0.0.1` would otherwise pass as loopback.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// One entry of `params.trusted_proxies`: an address or a CIDR.
///
/// An address without a prefix is a host (`/32`, `/128`). Host bits under the
/// prefix are masked off at parse, so `192.0.2.7/24` and `192.0.2.0/24` are the
/// same entry. A v4-mapped IPv6 entry of prefix 96 or longer is kept as the
/// IPv4 range it names, because the addresses it is compared with are
/// normalised the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProxyNet {
    net: IpAddr,
    prefix: u8,
}

impl ProxyNet {
    /// Whether `ip` lies inside this range. A v4-mapped IPv6 address is
    /// judged as the IPv4 address it carries.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.net, unmap(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                u32::from(ip) & mask32(self.prefix) == u32::from(net)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                u128::from(ip) & mask128(self.prefix) == u128::from(net)
            }
            // An IPv4 range never holds an IPv6 address, and the other way round.
            _ => false,
        }
    }

    /// The network address, with the host bits masked off.
    pub fn network(&self) -> IpAddr {
        self.net
    }

    /// The prefix length.
    pub fn prefix(&self) -> u8 {
        self.prefix
    }
}

/// Parse `params.trusted_proxies`.
///
/// The refusal names the entry by index and echoes it — an address is not a
/// secret, and "entry 3 is wrong" without the value sends the operator
/// counting.
pub fn parse_trusted_proxies(entries: &[String]) -> Result<Vec<ProxyNet>, String> {
    entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            parse_one(entry).ok_or_else(|| {
                format!(
                    "trusted_proxies[{i}]: {entry:?} is not an IP address or CIDR (an address, \
                     or an address with /0..=32 for IPv4, /0..=128 for IPv6)"
                )
            })
        })
        .collect()
}

/// One entry, or `None`. Strict: no whitespace, no sign on the prefix, no
/// host name — an entry that needed guessing would be one the operator did
/// not write.
fn parse_one(entry: &str) -> Option<ProxyNet> {
    let (addr, prefix) = match entry.split_once('/') {
        Some((addr, bits)) => {
            if bits.is_empty() || bits.len() > 3 || !bits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (addr, Some(bits.parse::<u8>().ok()?))
        }
        None => (entry, None),
    };
    let ip: IpAddr = addr.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let prefix = prefix.unwrap_or(max);
    if prefix > max {
        return None;
    }
    Some(ProxyNet::masked(ip, prefix))
}

impl ProxyNet {
    /// `ip/prefix` with the host bits cleared; a v4-mapped IPv6 range of
    /// prefix 96 or longer becomes the IPv4 range it names.
    fn masked(ip: IpAddr, prefix: u8) -> Self {
        match ip {
            IpAddr::V4(v4) => Self {
                net: IpAddr::V4(Ipv4Addr::from(u32::from(v4) & mask32(prefix))),
                prefix,
            },
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) if prefix >= 96 => Self::masked(IpAddr::V4(v4), prefix - 96),
                _ => Self {
                    net: IpAddr::V6(Ipv6Addr::from(u128::from(v6) & mask128(prefix))),
                    prefix,
                },
            },
        }
    }
}

/// The IPv4 address a v4-mapped IPv6 address carries, or the address itself.
fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

/// The netmask of an IPv4 prefix. `checked_shl` is the `/0` guard: a shift
/// by the full width is an overflow, and `/0` is the mask that keeps nothing.
fn mask32(prefix: u8) -> u32 {
    u32::MAX
        .checked_shl(32 - u32::from(prefix.min(32)))
        .unwrap_or(0)
}

/// The netmask of an IPv6 prefix, with the same `/0` guard.
fn mask128(prefix: u8) -> u128 {
    u128::MAX
        .checked_shl(128 - u32::from(prefix.min(128)))
        .unwrap_or(0)
}

/// The default list: `127.0.0.0/8` and `::1/128` (R-AG-1).
///
/// A reverse proxy on the same host needs no configuration; one on another
/// host is listed by its address. Fail-closed, the stance the peer mount
/// already takes on a missing header.
pub fn loopback_only() -> Vec<ProxyNet> {
    vec![
        ProxyNet::masked(IpAddr::V4(Ipv4Addr::LOCALHOST), 8),
        ProxyNet::masked(IpAddr::V6(Ipv6Addr::LOCALHOST), 128),
    ]
}

/// The list a mount judges with: the default when the key is absent, the
/// parsed entries otherwise. An explicit empty list trusts nobody.
pub fn trusted_proxies_or_default(entries: Option<&[String]>) -> Result<Vec<ProxyNet>, String> {
    match entries {
        None => Ok(loopback_only()),
        Some(list) => parse_trusted_proxies(list),
    }
}

/// Whether a connection from `ip` may name an identity. An empty list admits
/// nobody.
pub fn admits(list: &[ProxyNet], ip: IpAddr) -> bool {
    list.iter().any(|n| n.contains(ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nets(entries: &[&str]) -> Vec<ProxyNet> {
        let owned: Vec<String> = entries.iter().map(|s| s.to_string()).collect();
        match parse_trusted_proxies(&owned) {
            Ok(n) => n,
            Err(e) => panic!("{entries:?} must parse: {e}"),
        }
    }

    fn ip(s: &str) -> IpAddr {
        match s.parse() {
            Ok(ip) => ip,
            Err(e) => panic!("{s:?} is an address in this test: {e}"),
        }
    }

    #[test]
    fn a_bare_address_is_a_host() {
        let l = nets(&["192.0.2.1", "2001:db8::1"]);
        assert!(admits(&l, ip("192.0.2.1")));
        assert!(!admits(&l, ip("192.0.2.2")), "/32, not a range");
        assert!(admits(&l, ip("2001:db8::1")));
        assert!(!admits(&l, ip("2001:db8::2")), "/128, not a range");
        assert_eq!(l[0].prefix(), 32);
        assert_eq!(l[1].prefix(), 128);
    }

    #[test]
    fn a_host_prefix_a_class_prefix_and_the_zero_prefix() {
        let host = nets(&["192.0.2.7/32"]);
        assert!(admits(&host, ip("192.0.2.7")));
        assert!(!admits(&host, ip("192.0.2.8")));
        let eight = nets(&["127.0.0.0/8"]);
        assert!(admits(&eight, ip("127.255.1.2")));
        assert!(!admits(&eight, ip("192.0.2.1")));
        // Right at the edge of the prefix, inside a documentation net: the last
        // address in and the first address out. A mask one bit short (/24) or
        // one bit long (/26) turns one of the two lines red; 127/8 against
        // 192.0.2.1 differs in the first bit and cannot tell (review of the
        // #812 fix strand, Minor 3).
        let half = nets(&["192.0.2.0/25"]);
        assert!(admits(&half, ip("192.0.2.127")), "last address of the /25");
        assert!(!admits(&half, ip("192.0.2.128")), "first address past it");
        let all = nets(&["0.0.0.0/0"]);
        assert!(admits(&all, ip("203.0.113.9")), "/0 is every IPv4 address");
        assert!(
            !admits(&all, ip("2001:db8::1")),
            "an IPv4 range never holds an IPv6 address"
        );
        let all6 = nets(&["::/0"]);
        assert!(
            admits(&all6, ip("2001:db8::1")),
            "::/0 is every IPv6 address"
        );
    }

    #[test]
    fn host_bits_under_the_prefix_are_masked() {
        let l = nets(&["127.1.2.3/8", "2001:db8:ffff::1/32"]);
        assert_eq!(l[0].network(), ip("127.0.0.0"));
        assert_eq!(l[1].network(), ip("2001:db8::"));
        assert!(admits(&l, ip("127.200.0.1")));
        assert!(admits(&l, ip("2001:db8:1::5")));
    }

    #[test]
    fn loopback_is_the_default_and_nothing_else() {
        let d = loopback_only();
        for yes in ["127.0.0.1", "127.0.0.2", "127.255.255.254", "::1"] {
            assert!(admits(&d, ip(yes)), "{yes} is loopback");
        }
        for no in ["198.51.100.1", "192.0.2.1", "::2", "2001:db8::1", "0.0.0.0"] {
            assert!(!admits(&d, ip(no)), "{no} is not loopback");
        }
        assert_eq!(
            trusted_proxies_or_default(None).expect("the default parses"),
            d,
            "no key means the default"
        );
    }

    #[test]
    fn an_empty_list_admits_nobody() {
        assert!(!admits(&[], ip("127.0.0.1")));
        let explicit = trusted_proxies_or_default(Some(&[])).expect("an empty list parses");
        assert!(
            explicit.is_empty(),
            "an explicit empty list is not the default"
        );
        assert!(!admits(&explicit, ip("127.0.0.1")));
    }

    /// A listener bound to `[::]` reports an IPv4 client as `::ffff:a.b.c.d`;
    /// it must match the IPv4 range the operator wrote.
    #[test]
    fn a_v4_mapped_address_matches_its_v4_range() {
        // At the edge of the prefix, as above: a mapped address is judged by the
        // same mask, not by a looser one.
        let l = nets(&["198.51.100.0/25"]);
        assert!(admits(&l, ip("::ffff:198.51.100.127")));
        assert!(!admits(&l, ip("::ffff:198.51.100.128")));
        assert!(!admits(&l, ip("::ffff:203.0.113.7")));
        assert!(
            admits(&loopback_only(), ip("::ffff:127.0.0.1")),
            "a mapped loopback client is loopback"
        );
        let mapped_entry = nets(&["::ffff:192.0.2.0/120"]);
        assert!(
            admits(&mapped_entry, ip("192.0.2.200")),
            "a mapped entry is the IPv4 range it names"
        );
        assert_eq!(mapped_entry[0].network(), ip("192.0.2.0"));
        assert_eq!(mapped_entry[0].prefix(), 24);
    }

    /// NAT64 and 6to4 embed an IPv4 address too, but a peer address in those
    /// ranges is a remote host; unfolding it would let `2002:7f00:1::` pass as
    /// `127.0.0.1`.
    #[test]
    fn only_the_mapped_form_is_unfolded() {
        let d = loopback_only();
        assert!(!admits(&d, ip("2002:7f00:1::1")), "6to4 is not loopback");
        assert!(!admits(&d, ip("64:ff9b::7f00:1")), "NAT64 is not loopback");
    }

    #[test]
    fn a_refusal_names_the_entry_by_index_and_value() {
        for (bad, i) in [
            ("192.0.2.0/33", 1),
            ("::/129", 1),
            ("proxy.example", 1),
            ("192.0.2.0/", 1),
            ("192.0.2.0/+8", 1),
            ("/8", 1),
            (" 192.0.2.1", 1),
            ("", 1),
        ] {
            let entries = vec!["127.0.0.1".to_string(), bad.to_string()];
            let err = parse_trusted_proxies(&entries).expect_err(bad);
            assert!(
                err.starts_with(&format!(
                    "trusted_proxies[{i}]: {bad:?} is not an IP address or CIDR"
                )),
                "{bad:?}: {err}"
            );
        }
    }
}
