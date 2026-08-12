//! Pure address logic for `orbital ip`.
//!
//! Everything here works on plain data, so the tests feed fixtures instead of
//! touching real network interfaces.

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use crate::core::version::user_agent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Family::V4 => "IPv4",
            Family::V6 => "IPv6",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpEntry {
    /// Network interface name, e.g. `en0`, `eth0`.
    pub iface: String,
    /// The assigned address.
    pub address: String,
    pub family: Family,
}

/// One interface address as reported by the OS: name, address, and whether it
/// is a loopback (which the TypeScript version called "internal").
pub type RawIface = (String, IpAddr, bool);

/// Collect non-internal (loopback excluded) IP addresses for every interface.
pub fn collect_ips(ifaces: impl IntoIterator<Item = RawIface>) -> Vec<IpEntry> {
    ifaces
        .into_iter()
        .filter(|(_, _, loopback)| !*loopback)
        .map(|(iface, address, _)| IpEntry {
            iface,
            address: address.to_string(),
            family: match address {
                IpAddr::V4(_) => Family::V4,
                IpAddr::V6(_) => Family::V6,
            },
        })
        .collect()
}

/// Read this machine's interfaces. Returns an empty list when they can't be
/// enumerated, so callers render "no interfaces" instead of failing.
pub fn local_ips() -> Vec<IpEntry> {
    let ifaces = match if_addrs::get_if_addrs() {
        Ok(ifaces) => ifaces,
        Err(_) => return Vec::new(),
    };
    // Kept in the order the OS reports them: that ordering reflects adapter
    // priority, and `primary_ip` / `--local` pick the first IPv4 from it.
    collect_ips(ifaces.into_iter().map(|iface| {
        let (ip, loopback) = (iface.ip(), iface.is_loopback());
        (iface.name, ip, loopback)
    }))
}

/// The single most useful "this machine's IP" — the first non-internal IPv4,
/// or the first IPv6 if no IPv4 exists. `None` when offline.
pub fn primary_ip(entries: &[IpEntry]) -> Option<&IpEntry> {
    entries
        .iter()
        .find(|e| e.family == Family::V4)
        .or_else(|| entries.first())
}

/// The first non-internal IPv4 — this machine's address on the LAN.
pub fn local_ipv4(entries: &[IpEntry]) -> Option<&IpEntry> {
    entries.iter().find(|e| e.family == Family::V4)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IpFlags {
    /// `--public`/`-p`: fetch this machine's public IP.
    pub public: bool,
    /// `--local`/`-l`: print just the LAN IPv4, plain (script-friendly).
    pub local: bool,
}

/// Parse the option flags `orbital ip` accepts from the forwarded CLI tokens.
pub fn parse_ip_flags(args: &[String]) -> IpFlags {
    let has = |names: [&str; 2]| args.iter().any(|a| names.contains(&a.as_str()));
    IpFlags {
        public: has(["--public", "-p"]),
        local: has(["--local", "-l"]),
    }
}

/// Loose check that a string looks like an IPv4 or IPv6 address.
pub fn looks_like_ip(value: &str) -> bool {
    value.parse::<IpAddr>().is_ok()
}

/// Fetch this machine's public IP from an external echo service. Returns `None`
/// on any network/parse error so callers can render a friendly message.
pub fn public_ip() -> Option<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();

    let text = agent
        .get("https://api.ipify.org")
        .header("User-Agent", user_agent())
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;

    let text = text.trim().to_string();
    looks_like_ip(&text).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn fixture() -> Vec<RawIface> {
        vec![
            ("lo0".to_string(), ip("127.0.0.1"), true),
            ("en0".to_string(), ip("fe80::1"), false),
            ("en0".to_string(), ip("192.168.1.5"), false),
        ]
    }

    #[test]
    fn excludes_internal_addresses() {
        let ips = collect_ips(fixture());
        assert!(!ips.iter().any(|e| e.address == "127.0.0.1"));
    }

    #[test]
    fn returns_both_ipv4_and_ipv6_non_internal_addresses() {
        let ips = collect_ips(fixture());
        assert_eq!(
            ips,
            vec![
                IpEntry {
                    iface: "en0".into(),
                    address: "fe80::1".into(),
                    family: Family::V6
                },
                IpEntry {
                    iface: "en0".into(),
                    address: "192.168.1.5".into(),
                    family: Family::V4
                },
            ]
        );
    }

    #[test]
    fn handles_no_interfaces() {
        assert!(collect_ips(Vec::new()).is_empty());
    }

    #[test]
    fn primary_prefers_the_first_ipv4() {
        let ips = collect_ips(fixture());
        assert_eq!(primary_ip(&ips).unwrap().address, "192.168.1.5");
    }

    #[test]
    fn primary_falls_back_to_the_first_entry_without_ipv4() {
        let ips = collect_ips(vec![("en0".to_string(), ip("fe80::1"), false)]);
        assert_eq!(primary_ip(&ips).unwrap().family, Family::V6);
    }

    #[test]
    fn primary_of_an_empty_list_is_nothing() {
        assert!(primary_ip(&[]).is_none());
    }

    #[test]
    fn local_ipv4_ignores_ipv6() {
        let ips = collect_ips(fixture());
        assert_eq!(local_ipv4(&ips).unwrap().address, "192.168.1.5");
    }

    #[test]
    fn local_ipv4_is_nothing_when_only_ipv6_exists() {
        let ips = collect_ips(vec![("en0".to_string(), ip("fe80::1"), false)]);
        assert!(local_ipv4(&ips).is_none());
    }

    #[test]
    fn flags_default_to_off() {
        assert_eq!(
            parse_ip_flags(&[]),
            IpFlags {
                public: false,
                local: false
            }
        );
    }

    #[test]
    fn recognizes_long_and_short_flag_forms() {
        let args = |a: &str| vec![a.to_string()];
        assert_eq!(
            parse_ip_flags(&args("--public")),
            IpFlags {
                public: true,
                local: false
            }
        );
        assert_eq!(
            parse_ip_flags(&args("-p")),
            IpFlags {
                public: true,
                local: false
            }
        );
        assert_eq!(
            parse_ip_flags(&args("--local")),
            IpFlags {
                public: false,
                local: true
            }
        );
        assert_eq!(
            parse_ip_flags(&args("-l")),
            IpFlags {
                public: false,
                local: true
            }
        );
    }

    #[test]
    fn recognizes_addresses_but_not_prose() {
        assert!(looks_like_ip("203.0.113.7"));
        assert!(looks_like_ip("fe80::1"));
        assert!(!looks_like_ip("<html>error</html>"));
        assert!(!looks_like_ip(""));
    }

    #[test]
    fn families_render_the_way_the_listing_prints_them() {
        assert_eq!(Family::V4.to_string(), "IPv4");
        assert_eq!(Family::V6.to_string(), "IPv6");
    }
}
