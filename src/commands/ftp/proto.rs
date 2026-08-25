//! Pure protocol logic for `orbital ftp` — argument parsing, reply and listing
//! formatting, virtual-path jailing, and UTC date math.
//!
//! No sockets and no filesystem in this module: everything works on plain data
//! so every protocol decision is a function the tests can call directly. The
//! socket side lives in `server.rs`.

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

/// Default control port. 21 needs root/admin on most systems, so use the
/// conventional unprivileged FTP port instead.
pub const DEFAULT_PORT: u16 = 2121;

/// Options `orbital ftp` accepts from the forwarded CLI tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FtpArgs {
    pub port: u16,
    /// Allow STOR/DELE/MKD/RNTO — off by default, so sharing is read-only.
    pub writable: bool,
}

/// Parse the tokens after `orbital ftp`: `--write`/`-w` enables uploads, and
/// the port comes from `--port N`, `--port=N`, or a bare number (matching
/// `orbital serve 8080`). Unknown tokens are ignored.
pub fn parse_ftp_args(args: &[String]) -> FtpArgs {
    let mut parsed = FtpArgs {
        port: DEFAULT_PORT,
        writable: false,
    };
    let mut want_port_value = false;

    for arg in args {
        if want_port_value {
            want_port_value = false;
            if let Some(port) = parse_port_token(arg) {
                parsed.port = port;
            }
            continue;
        }
        match arg.as_str() {
            "--write" | "-w" | "--rw" => parsed.writable = true,
            "--port" | "-p" => want_port_value = true,
            other => {
                if let Some(value) = other.strip_prefix("--port=") {
                    if let Some(port) = parse_port_token(value) {
                        parsed.port = port;
                    }
                } else if let Some(port) = parse_port_token(other) {
                    parsed.port = port;
                }
            }
        }
    }
    parsed
}

fn parse_port_token(token: &str) -> Option<u16> {
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    token
        .parse::<u32>()
        .ok()
        .filter(|n| (1..=65535).contains(n))
        .map(|n| n as u16)
}

/// Drop leading Telnet control bytes. The classic `ftp` client prefixes ABOR
/// with IAC sequences (bytes ≥ 0xF0); they are noise to us.
pub fn strip_telnet(mut bytes: &[u8]) -> &[u8] {
    while let Some(&first) = bytes.first() {
        if first < 0xf0 {
            break;
        }
        bytes = &bytes[1..];
    }
    bytes
}

/// Split a control line into an uppercased verb and its argument. The argument
/// keeps its case — file names are case-sensitive.
pub fn parse_command(line: &str) -> (String, String) {
    let line = line.trim_end_matches(['\r', '\n']);
    match line.split_once(' ') {
        Some((verb, arg)) => (verb.to_ascii_uppercase(), arg.to_string()),
        None => (line.to_ascii_uppercase(), String::new()),
    }
}

/// Drop `ls`-style flags some clients prepend to LIST/NLST (`LIST -la`).
pub fn strip_list_flags(arg: &str) -> &str {
    let mut rest = arg.trim_start();
    while rest.starts_with('-') {
        match rest.split_once(' ') {
            Some((_, tail)) => rest = tail.trim_start(),
            None => return "",
        }
    }
    rest
}

/// Resolve a client-supplied path against the session's virtual cwd into a
/// list of plain path components — the jail that keeps every request inside
/// the shared root.
///
/// `..` pops (clamped at the root, so `CWD ..` can never escape), `/` and `\`
/// both separate, and a component containing `:` or NUL is refused outright: a
/// drive letter (`C:`) or NTFS stream would otherwise let `Path::join` wander
/// off the root on Windows.
pub fn resolve_virtual(cwd: &[String], arg: &str) -> Option<Vec<String>> {
    let mut parts: Vec<String> = if arg.starts_with('/') || arg.starts_with('\\') {
        Vec::new()
    } else {
        cwd.to_vec()
    };
    for segment in arg.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            name => {
                if name.contains(':') || name.contains('\0') {
                    return None;
                }
                parts.push(name.to_string());
            }
        }
    }
    Some(parts)
}

/// Render a virtual path the way FTP shows it: `/` for the root, `/a/b` below.
pub fn virtual_display(parts: &[String]) -> String {
    format!("/{}", parts.join("/"))
}

/// Map a virtual path onto the real filesystem under `root`. Components are
/// single names by construction, but keep the same belt-and-braces guarantee
/// `serve` makes: nothing outside the root ever resolves.
pub fn real_path(root: &Path, parts: &[String]) -> Option<PathBuf> {
    let path = parts
        .iter()
        .fold(root.to_path_buf(), |p, part| p.join(part));
    path.starts_with(root).then_some(path)
}

/// RFC 959 quotes the path in `257` replies, doubling embedded quotes.
fn quoted(parts: &[String]) -> String {
    virtual_display(parts).replace('"', "\"\"")
}

pub fn pwd_reply(parts: &[String]) -> String {
    format!("257 \"{}\" is the current directory.", quoted(parts))
}

pub fn mkd_reply(parts: &[String]) -> String {
    format!("257 \"{}\" created.", quoted(parts))
}

/// One directory entry, read at the edge and formatted here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryMeta {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix seconds, clamped to zero for pre-epoch or unreadable times.
    pub mtime_secs: i64,
}

/// A name is one line of a listing, so line breaks inside it would corrupt
/// the whole listing for the client.
fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| if c == '\r' || c == '\n' { '?' } else { c })
        .collect()
}

/// One `ls -l`-style line, which is what LIST clients parse. Permissions show
/// what the server actually allows, not what the OS would.
pub fn list_line(entry: &EntryMeta, now_secs: i64, writable: bool) -> String {
    let perms = match (entry.is_dir, writable) {
        (true, true) => "drwxr-xr-x",
        (true, false) => "dr-xr-xr-x",
        (false, true) => "-rw-r--r--",
        (false, false) => "-r--r--r--",
    };
    format!(
        "{perms} 1 ftp ftp {size:>12} {date} {name}",
        size = entry.size,
        date = format_list_date(entry.mtime_secs, now_secs),
        name = safe_name(&entry.name),
    )
}

/// One MLSD fact line — the machine-readable listing modern clients prefer.
pub fn mlsd_line(entry: &EntryMeta) -> String {
    let stamp = format_mdtm(entry.mtime_secs);
    if entry.is_dir {
        format!("type=dir;modify={stamp}; {}", safe_name(&entry.name))
    } else {
        format!(
            "type=file;size={};modify={stamp}; {}",
            entry.size,
            safe_name(&entry.name)
        )
    }
}

/// One NLST line: just the name.
pub fn nlst_line(entry: &EntryMeta) -> String {
    safe_name(&entry.name)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `ls` shows the time for recent files and the year otherwise; six months is
/// its traditional cutoff.
const RECENT_SECS: i64 = 180 * 24 * 60 * 60;

/// Unix seconds → UTC (year, month, day, hour, minute, second), via Howard
/// Hinnant's civil-date algorithm. UTC everywhere: MDTM and MLSD facts are
/// defined as UTC, and it spares us a timezone database.
pub fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, minute, second) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era   [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year  [0, 365]
    let mp = (5 * doy + 2) / 153; // month, March-based  [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    (
        year,
        month as u32,
        day as u32,
        hour as u32,
        minute as u32,
        second as u32,
    )
}

/// The date column of a LIST line: `Aug 25 10:04` for recent files,
/// `Aug 25  2024` otherwise — both 12 columns, like `ls`.
pub fn format_list_date(mtime_secs: i64, now_secs: i64) -> String {
    let (year, month, day, hour, minute, _) = civil_from_unix(mtime_secs);
    let month = MONTHS[(month - 1) as usize];
    let recent = now_secs - mtime_secs < RECENT_SECS && mtime_secs - now_secs < 3_600;
    if recent {
        format!("{month} {day:>2} {hour:02}:{minute:02}")
    } else {
        format!("{month} {day:>2}  {year}")
    }
}

/// `YYYYMMDDHHMMSS` in UTC — the MDTM reply and the MLSD `modify` fact.
pub fn format_mdtm(secs: i64) -> String {
    let (year, month, day, hour, minute, second) = civil_from_unix(secs);
    format!("{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}")
}

/// The `227` reply for PASV: the address the client should dial, comma-packed.
pub fn pasv_reply(ip: Ipv4Addr, port: u16) -> String {
    let o = ip.octets();
    format!(
        "227 Entering Passive Mode ({},{},{},{},{},{}).",
        o[0],
        o[1],
        o[2],
        o[3],
        port >> 8,
        port & 0xff
    )
}

/// The `229` reply for EPSV — the client reuses the control host, so only the
/// port travels.
pub fn epsv_reply(port: u16) -> String {
    format!("229 Entering Extended Passive Mode (|||{port}|)")
}

/// Parse the `h1,h2,h3,h4,p1,p2` argument of PORT.
pub fn parse_host_port(arg: &str) -> Option<(Ipv4Addr, u16)> {
    let mut numbers = [0u8; 6];
    let mut count = 0;
    for token in arg.split(',') {
        if count == 6 {
            return None;
        }
        numbers[count] = token.trim().parse::<u8>().ok()?;
        count += 1;
    }
    if count != 6 {
        return None;
    }
    let port = (u16::from(numbers[4]) << 8) | u16::from(numbers[5]);
    if port == 0 {
        return None;
    }
    Some((
        Ipv4Addr::new(numbers[0], numbers[1], numbers[2], numbers[3]),
        port,
    ))
}

/// Parse the `|1|host|port|` argument of EPRT. The first character is the
/// delimiter (the RFC allows any); protocol `1` is IPv4 and `2` is IPv6.
pub fn parse_eprt(arg: &str) -> Option<(IpAddr, u16)> {
    let delimiter = arg.chars().next()?;
    let mut fields = arg.split(delimiter);
    fields.next()?; // the empty slot before the leading delimiter
    let protocol = fields.next()?;
    let host = fields.next()?;
    let port = fields.next()?;
    if protocol != "1" && protocol != "2" {
        return None;
    }
    let ip: IpAddr = host.parse().ok()?;
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    Some((ip, port))
}

/// The FEAT block — what we advertise so clients pick modern verbs (UTF-8
/// names, EPSV, resume, and MLSD listings).
pub fn feat_lines() -> Vec<String> {
    [
        "211-Features:",
        " UTF8",
        " EPSV",
        " REST STREAM",
        " SIZE",
        " MDTM",
        " MLST type*;size*;modify*;",
        "211 End.",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtpUrls {
    /// Loopback URL for this machine.
    pub local: String,
    /// LAN URL other devices can reach, or `None` when offline.
    pub network: Option<String>,
}

/// The URLs the server is reachable at — localhost plus the LAN IPv4.
pub fn ftp_urls(port: u16, lan_ipv4: Option<&str>) -> FtpUrls {
    FtpUrls {
        local: format!("ftp://localhost:{port}"),
        network: lan_ipv4.map(|ip| format!("ftp://{ip}:{port}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn parts(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_to_a_read_only_server_on_2121() {
        assert_eq!(
            parse_ftp_args(&[]),
            FtpArgs {
                port: DEFAULT_PORT,
                writable: false
            }
        );
    }

    #[test]
    fn accepts_every_port_spelling() {
        assert_eq!(parse_ftp_args(&args(&["2222"])).port, 2222);
        assert_eq!(parse_ftp_args(&args(&["--port", "2222"])).port, 2222);
        assert_eq!(parse_ftp_args(&args(&["--port=2222"])).port, 2222);
        assert_eq!(parse_ftp_args(&args(&["-p", "2222"])).port, 2222);
    }

    #[test]
    fn recognizes_the_write_flags() {
        assert!(parse_ftp_args(&args(&["--write"])).writable);
        assert!(parse_ftp_args(&args(&["-w"])).writable);
        assert!(parse_ftp_args(&args(&["--rw"])).writable);
        assert!(parse_ftp_args(&args(&["--write", "2121"])).writable);
    }

    #[test]
    fn ignores_junk_and_invalid_ports() {
        let parsed = parse_ftp_args(&args(&["--mystery", "0", "99999", "notaport"]));
        assert_eq!(parsed.port, DEFAULT_PORT);
        assert!(!parsed.writable);
    }

    #[test]
    fn uppercases_the_verb_but_not_the_argument() {
        assert_eq!(
            parse_command("retr My File.TXT\r\n"),
            ("RETR".to_string(), "My File.TXT".to_string())
        );
        assert_eq!(parse_command("PWD"), ("PWD".to_string(), String::new()));
    }

    #[test]
    fn strips_leading_telnet_bytes() {
        assert_eq!(strip_telnet(&[0xff, 0xf4, 0xff, 0xf2, b'A']), b"A");
        assert_eq!(strip_telnet(b"ABOR"), b"ABOR");
    }

    #[test]
    fn strips_ls_flags_from_list_arguments() {
        assert_eq!(strip_list_flags("-la"), "");
        assert_eq!(strip_list_flags("-a sub"), "sub");
        assert_eq!(strip_list_flags("sub"), "sub");
        // A name containing a dash mid-way is a name, not a flag.
        assert_eq!(strip_list_flags("my -file"), "my -file");
        assert_eq!(strip_list_flags(""), "");
    }

    #[test]
    fn resolves_relative_and_absolute_paths() {
        let cwd = parts(&["a"]);
        assert_eq!(
            resolve_virtual(&cwd, "b/c").unwrap(),
            parts(&["a", "b", "c"])
        );
        assert_eq!(resolve_virtual(&cwd, "/b").unwrap(), parts(&["b"]));
        assert_eq!(resolve_virtual(&cwd, "").unwrap(), parts(&["a"]));
        assert_eq!(resolve_virtual(&cwd, ".").unwrap(), parts(&["a"]));
    }

    #[test]
    fn clamps_traversal_at_the_root() {
        let cwd = parts(&["a"]);
        assert_eq!(resolve_virtual(&cwd, "..").unwrap(), Vec::<String>::new());
        assert_eq!(
            resolve_virtual(&[], "../../etc/passwd").unwrap(),
            parts(&["etc", "passwd"])
        );
    }

    #[test]
    fn treats_backslashes_as_separators() {
        assert_eq!(resolve_virtual(&[], "a\\b").unwrap(), parts(&["a", "b"]));
        assert_eq!(
            resolve_virtual(&parts(&["a"]), "\\b").unwrap(),
            parts(&["b"])
        );
    }

    #[test]
    fn refuses_drive_letters_streams_and_nul() {
        assert!(resolve_virtual(&[], "C:/Windows").is_none());
        assert!(resolve_virtual(&[], "file.txt:stream").is_none());
        assert!(resolve_virtual(&[], "a\0b").is_none());
    }

    #[test]
    fn maps_virtual_paths_onto_the_root() {
        let root = Path::new("/srv/share");
        assert_eq!(
            real_path(root, &parts(&["a", "b"])).unwrap(),
            root.join("a").join("b")
        );
        assert_eq!(real_path(root, &[]).unwrap(), root);
    }

    #[test]
    fn displays_the_root_as_a_single_slash() {
        assert_eq!(virtual_display(&[]), "/");
        assert_eq!(virtual_display(&parts(&["a", "b"])), "/a/b");
    }

    #[test]
    fn doubles_quotes_in_257_replies() {
        assert_eq!(pwd_reply(&[]), "257 \"/\" is the current directory.");
        assert_eq!(
            pwd_reply(&parts(&["a\"b"])),
            "257 \"/a\"\"b\" is the current directory."
        );
        assert_eq!(mkd_reply(&parts(&["box"])), "257 \"/box\" created.");
    }

    #[test]
    fn converts_unix_time_to_utc_civil_time() {
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        // A classic vector: 2001-09-09 01:46:40 UTC.
        assert_eq!(civil_from_unix(1_000_000_000), (2001, 9, 9, 1, 46, 40));
        // A leap day: 2000-02-29 00:00:00 UTC.
        assert_eq!(civil_from_unix(951_782_400), (2000, 2, 29, 0, 0, 0));
    }

    #[test]
    fn mdtm_stamps_are_utc_and_zero_padded() {
        assert_eq!(format_mdtm(0), "19700101000000");
        assert_eq!(format_mdtm(1_000_000_000), "20010909014640");
    }

    #[test]
    fn recent_files_show_a_time_and_old_ones_a_year() {
        let now = 1_000_000_000; // 2001-09-09 01:46:40
        assert_eq!(format_list_date(now - 60, now), "Sep  9 01:45");
        assert_eq!(format_list_date(0, now), "Jan  1  1970");
        // A file dated well into the future gets the year too.
        assert_eq!(format_list_date(now + 86_400 * 400, now), "Oct 14  2002");
    }

    fn entry(name: &str, is_dir: bool, size: u64) -> EntryMeta {
        EntryMeta {
            name: name.to_string(),
            is_dir,
            size,
            mtime_secs: 1_000_000_000,
        }
    }

    #[test]
    fn list_lines_show_the_servers_real_permissions() {
        let now = 1_000_000_000;
        let read_only = list_line(&entry("a.txt", false, 42), now, false);
        assert!(read_only.starts_with("-r--r--r--"), "{read_only}");
        assert!(read_only.ends_with(" a.txt"));
        assert!(read_only.contains(" 42 "));

        let writable = list_line(&entry("a.txt", false, 42), now, true);
        assert!(writable.starts_with("-rw-"), "{writable}");

        let dir = list_line(&entry("sub", true, 0), now, false);
        assert!(dir.starts_with("dr-x"), "{dir}");
    }

    #[test]
    fn listing_names_cannot_smuggle_line_breaks() {
        let sneaky = entry("a\r\nb", false, 1);
        assert!(list_line(&sneaky, 0, false).ends_with("a??b"));
        assert!(nlst_line(&sneaky) == "a??b");
    }

    #[test]
    fn mlsd_lines_carry_type_size_and_modify_facts() {
        assert_eq!(
            mlsd_line(&entry("a.txt", false, 42)),
            "type=file;size=42;modify=20010909014640; a.txt"
        );
        assert_eq!(
            mlsd_line(&entry("sub", true, 0)),
            "type=dir;modify=20010909014640; sub"
        );
    }

    #[test]
    fn packs_the_pasv_address_into_comma_octets() {
        assert_eq!(
            pasv_reply(Ipv4Addr::new(192, 168, 1, 5), 5001),
            "227 Entering Passive Mode (192,168,1,5,19,137)."
        );
    }

    #[test]
    fn epsv_replies_with_just_the_port() {
        assert_eq!(
            epsv_reply(5001),
            "229 Entering Extended Passive Mode (|||5001|)"
        );
    }

    #[test]
    fn parses_port_arguments() {
        assert_eq!(
            parse_host_port("192,168,1,5,19,137"),
            Some((Ipv4Addr::new(192, 168, 1, 5), 5001))
        );
        assert_eq!(parse_host_port("1,2,3,4,5"), None); // too short
        assert_eq!(parse_host_port("1,2,3,4,5,6,7"), None); // too long
        assert_eq!(parse_host_port("256,2,3,4,5,6"), None); // not an octet
        assert_eq!(parse_host_port("1,2,3,4,0,0"), None); // port zero
    }

    #[test]
    fn parses_eprt_arguments_with_any_delimiter() {
        assert_eq!(
            parse_eprt("|1|127.0.0.1|8080|"),
            Some(("127.0.0.1".parse().unwrap(), 8080))
        );
        assert_eq!(
            parse_eprt("!2!::1!8080!"),
            Some(("::1".parse().unwrap(), 8080))
        );
        assert_eq!(parse_eprt("|3|1.2.3.4|80|"), None); // unknown protocol
        assert_eq!(parse_eprt("|1|nonsense|80|"), None);
        assert_eq!(parse_eprt(""), None);
    }

    #[test]
    fn the_feat_block_is_a_well_formed_211_reply() {
        let lines = feat_lines();
        assert!(lines.first().unwrap().starts_with("211-"));
        assert!(lines.last().unwrap().starts_with("211 "));
        let block = lines.join("\n");
        assert!(block.contains("UTF8"));
        assert!(block.contains("MLST"));
        assert!(block.contains("EPSV"));
    }

    #[test]
    fn builds_local_and_network_urls() {
        assert_eq!(
            ftp_urls(2121, Some("192.168.1.42")),
            FtpUrls {
                local: "ftp://localhost:2121".into(),
                network: Some("ftp://192.168.1.42:2121".into()),
            }
        );
        assert!(ftp_urls(2121, None).network.is_none());
    }
}
