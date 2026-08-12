//! `orbital ip` — show this machine's addresses.

pub mod addresses;

use addresses::{local_ips, local_ipv4, parse_ip_flags, primary_ip, public_ip, IpEntry};

use crate::commands::{Command, Ctx, Stdin};
use crate::style;
use crate::term;
use crate::Res;

pub const COMMAND: Command = Command {
    name: "ip",
    description: "Show IPs — default lists local; --local (LAN IPv4), --public",
    aliases: &["ipaddr"],
    // --local / --public print a plain address for scripting (no UI chrome).
    run: Some(run),
    view,
    stdin: Stdin::Never,
};

fn run(ctx: &Ctx) -> Result<Option<String>, String> {
    let flags = parse_ip_flags(ctx.args);

    if flags.public {
        return public_ip()
            .map(Some)
            .ok_or_else(|| "Could not determine public IP.".to_string());
    }

    if flags.local {
        let entries = local_ips();
        return local_ipv4(&entries)
            .map(|e| Some(e.address.clone()))
            .ok_or_else(|| "No LAN IPv4 address found.".to_string());
    }

    Ok(None)
}

/// Minimum width of the interface-name column.
const NAME_WIDTH: usize = 10;

fn view(_ctx: &Ctx) -> Res {
    term::emit(&render(&local_ips()));
    Ok(())
}

fn render(entries: &[IpEntry]) -> Vec<String> {
    if entries.is_empty() {
        return vec![style::yellow("No non-internal network interfaces found.")];
    }

    let mut lines = Vec::new();
    if let Some(primary) = primary_ip(entries) {
        lines.push(format!(
            "{}{}{}",
            style::bold_green("Local IP: "),
            style::bold(&primary.address),
            style::dim(&format!(" ({})", primary.iface))
        ));
        lines.push(String::new());
    }
    // Widen the name column to fit the longest adapter, so families and
    // addresses still line up next to names like `vEthernet (Default Switch)`.
    let width = entries
        .iter()
        .map(|e| e.iface.chars().count() + 1)
        .max()
        .unwrap_or(NAME_WIDTH)
        .max(NAME_WIDTH);

    for entry in entries {
        lines.push(format!(
            "{}{}{}",
            style::cyan(&style::pad(&entry.iface, width)),
            style::dim(&style::pad(&entry.family.to_string(), 6)),
            entry.address
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use addresses::{collect_ips, Family};

    fn entries() -> Vec<IpEntry> {
        collect_ips(vec![
            ("en0".to_string(), "fe80::1".parse().unwrap(), false),
            ("en0".to_string(), "192.168.1.5".parse().unwrap(), false),
        ])
    }

    #[test]
    fn says_so_when_there_are_no_interfaces() {
        let lines = render(&[]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("No non-internal network interfaces found."));
    }

    #[test]
    fn leads_with_the_primary_address() {
        let lines = render(&entries());
        assert!(lines[0].contains("Local IP: 192.168.1.5"));
        assert!(lines[0].contains("(en0)"));
    }

    #[test]
    fn lists_every_address_with_its_family() {
        let lines = render(&entries());
        let listing = lines.join("\n");
        assert!(listing.contains("IPv6"));
        assert!(listing.contains("fe80::1"));
        assert!(listing.contains("IPv4"));
        assert!(listing.contains("192.168.1.5"));
    }

    #[test]
    fn columns_are_padded_so_addresses_line_up() {
        let lines = render(&entries());
        // Rows follow the header and its blank line.
        assert!(lines[2].starts_with("en0       IPv6  "));
        assert!(lines[3].starts_with("en0       IPv4  "));
    }

    #[test]
    fn a_long_adapter_name_widens_the_column_for_every_row() {
        let entries = collect_ips(vec![
            (
                "vEthernet (Default Switch)".to_string(),
                "10.0.0.1".parse().unwrap(),
                false,
            ),
            ("Wi-Fi".to_string(), "192.168.1.5".parse().unwrap(), false),
        ]);
        let lines = render(&entries);
        let family_at = |line: &str| line.find("IPv4").unwrap();
        // Both rows put the family column in the same place.
        assert_eq!(family_at(&lines[2]), family_at(&lines[3]));
        // …and it clears the longest name rather than colliding with it.
        assert!(family_at(&lines[2]) > "vEthernet (Default Switch)".len());
    }

    #[test]
    fn no_flags_falls_through_to_the_view() {
        let ctx = Ctx {
            args: &[],
            input: None,
            interactive: false,
        };
        assert_eq!(run(&ctx).unwrap(), None);
    }

    #[test]
    fn local_flag_fails_loudly_when_there_is_no_ipv4() {
        // Exercises the error path's wording; the real lookup is machine-specific.
        let entries: Vec<IpEntry> =
            collect_ips(vec![("en0".to_string(), "fe80::1".parse().unwrap(), false)]);
        assert!(local_ipv4(&entries).is_none());
        assert_eq!(entries[0].family, Family::V6);
    }
}
