//! Static-file serving logic for `orbital serve`, kept free of the HTTP server
//! itself so requests can be resolved and asserted on in tests.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::commands::ip::addresses::{local_ips, local_ipv4};

/// Default port when none is given, matching `python -m http.server`.
pub const DEFAULT_PORT: u16 = 8000;

/// Pick the port from the CLI args (first bare number), else the default.
pub fn resolve_serve_port(args: &[String], fallback: u16) -> u16 {
    args.iter()
        .find(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_digit()))
        .and_then(|token| token.parse::<u32>().ok())
        .filter(|n| *n >= 1 && *n <= 65535)
        .map(|n| n as u16)
        .unwrap_or(fallback)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeUrls {
    /// Loopback URL for this machine.
    pub local: String,
    /// LAN URL other devices can reach, or `None` when offline.
    pub network: Option<String>,
}

/// The URLs the server is reachable at — localhost plus the LAN IPv4.
pub fn serve_urls(port: u16, lan_ipv4: Option<&str>) -> ServeUrls {
    ServeUrls {
        local: format!("http://localhost:{port}"),
        network: lan_ipv4.map(|ip| format!("http://{ip}:{port}")),
    }
}

/// The URLs for this machine, reading its real interfaces.
pub fn current_serve_urls(port: u16) -> ServeUrls {
    let entries = local_ips();
    serve_urls(port, local_ipv4(&entries).map(|e| e.address.as_str()))
}

/// What a request resolves to inside the served root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The request tried to escape the root.
    Forbidden,
    NotFound,
    File(PathBuf),
    /// A directory with no `index.html`; render a listing for this URL path.
    Listing {
        dir: PathBuf,
        url_path: String,
    },
}

/// Decode `%XX` escapes. Invalid escapes are left as written, and invalid
/// UTF-8 is replaced rather than rejected.
pub fn decode_percent(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Normalise a request path to an absolute, traversal-free URL path.
///
/// Decoding happens first, so `%2e%2e%2f` can't smuggle a `..` past the check;
/// `..` segments are then resolved and clamped at the root, mirroring how the
/// TypeScript version's `new URL()` collapsed them before the guard ran.
pub fn normalize_url_path(raw: &str) -> String {
    let path = raw.split(['?', '#']).next().unwrap_or("");
    let decoded = decode_percent(path);

    let mut segments: Vec<String> = Vec::new();
    for segment in decoded.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other.to_string()),
        }
    }
    format!("/{}", segments.join("/"))
}

/// Resolve a request URL against the served root. Directories return their
/// `index.html` when present, otherwise a listing; missing paths are 404; any
/// path that would land outside `root` is refused.
pub fn resolve_target(root: &Path, url: &str) -> Target {
    let url_path = normalize_url_path(url);
    let relative = url_path.trim_start_matches('/');

    let mut target = root.to_path_buf();
    if !relative.is_empty() {
        target.push(relative);
    }

    // Defence in depth: normalisation already removed `..`, but a Windows drive
    // prefix (`/C:/…`) can still make `push` discard the root entirely.
    if !target.starts_with(root) || target.components().any(|c| c == Component::ParentDir) {
        return Target::Forbidden;
    }

    let metadata = match fs::metadata(&target) {
        Ok(metadata) => metadata,
        Err(_) => return Target::NotFound,
    };

    if metadata.is_dir() {
        let index = target.join("index.html");
        if fs::metadata(&index).map(|m| m.is_file()).unwrap_or(false) {
            return Target::File(index);
        }
        return Target::Listing {
            dir: target,
            url_path,
        };
    }

    Target::File(target)
}

/// One row of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// Read a directory into sorted listing entries: directories first, then
/// alphabetical — like a familiar file listing.
pub fn read_entries(dir: &Path) -> Vec<Entry> {
    let mut entries: Vec<Entry> = match fs::read_dir(dir) {
        Ok(reader) => reader
            .filter_map(|entry| entry.ok())
            .map(|entry| Entry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: entry.file_type().map(|t| t.is_dir()).unwrap_or(false),
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    entries
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Percent-encode the characters that would break an `href`.
fn encode_uri(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(*byte as char),
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                out.push(*byte as char)
            }
            b';' | b'/' | b'?' | b':' | b'@' | b'&' | b'=' | b'+' | b'$' | b',' | b'#' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Build the HTML index for a directory.
pub fn directory_listing_html(entries: &[Entry], url_path: &str) -> String {
    let base = if url_path.ends_with('/') {
        url_path.to_string()
    } else {
        format!("{url_path}/")
    };

    let mut rows = String::new();
    if url_path != "/" {
        rows.push_str(&format!(
            "<li><a href=\"{}\">../</a></li>",
            encode_uri(&format!("{base}.."))
        ));
    }
    for entry in entries {
        let name = if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };
        rows.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>",
            encode_uri(&format!("{base}{name}")),
            escape_html(&name)
        ));
    }

    let title = format!("Directory listing for {}", escape_html(url_path));
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head>\n\
         <body><h1>{title}</h1><ul>{rows}</ul></body></html>"
    )
}

/// Content type for a path, by extension.
pub fn mime_for(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match extension.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "txt" | "md" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// A throwaway directory tree for the resolver tests.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("orbital-serve-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn file(&self, name: &str, contents: &str) {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, contents).unwrap();
        }

        fn dir(&self, name: &str) {
            fs::create_dir_all(self.0.join(name)).unwrap();
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn uses_the_first_bare_number_arg() {
        assert_eq!(resolve_serve_port(&args(&["8080"]), DEFAULT_PORT), 8080);
        assert_eq!(
            resolve_serve_port(&args(&["--foo", "3000"]), DEFAULT_PORT),
            3000
        );
    }

    #[test]
    fn falls_back_to_the_default_when_absent_or_invalid() {
        assert_eq!(resolve_serve_port(&[], DEFAULT_PORT), 8000);
        assert_eq!(resolve_serve_port(&args(&["notaport"]), DEFAULT_PORT), 8000);
        assert_eq!(resolve_serve_port(&args(&["99999"]), DEFAULT_PORT), 8000); // out of range
        assert_eq!(resolve_serve_port(&args(&["0"]), DEFAULT_PORT), 8000);
    }

    #[test]
    fn builds_local_and_network_urls() {
        assert_eq!(
            serve_urls(8080, Some("192.168.1.42")),
            ServeUrls {
                local: "http://localhost:8080".into(),
                network: Some("http://192.168.1.42:8080".into()),
            }
        );
    }

    #[test]
    fn has_no_network_url_without_a_lan_ipv4() {
        assert!(serve_urls(8080, None).network.is_none());
    }

    #[test]
    fn normalizes_dot_segments_and_query_strings() {
        assert_eq!(normalize_url_path("/a/./b"), "/a/b");
        assert_eq!(normalize_url_path("/a/b/../c"), "/a/c");
        assert_eq!(normalize_url_path("/a//b"), "/a/b");
        assert_eq!(normalize_url_path("/file.txt?v=1"), "/file.txt");
        assert_eq!(normalize_url_path("/"), "/");
    }

    #[test]
    fn clamps_traversal_at_the_root() {
        assert_eq!(normalize_url_path("/../../etc/passwd"), "/etc/passwd");
        // Percent-encoded traversal is decoded before it is resolved.
        assert_eq!(
            normalize_url_path("/%2e%2e/%2e%2e/etc/passwd"),
            "/etc/passwd"
        );
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(decode_percent("/a%20b"), "/a b");
        assert_eq!(decode_percent("/caf%C3%A9"), "/café");
        // A stray `%` is left alone rather than eating the next characters.
        assert_eq!(decode_percent("100%"), "100%");
    }

    #[test]
    fn serves_a_file() {
        let tree = TempTree::new("file");
        tree.file("hello.txt", "hi there");
        assert_eq!(
            resolve_target(tree.path(), "/hello.txt"),
            Target::File(tree.path().join("hello.txt"))
        );
    }

    #[test]
    fn lists_a_directory_without_an_index() {
        let tree = TempTree::new("listing");
        tree.file("hello.txt", "hi");
        match resolve_target(tree.path(), "/") {
            Target::Listing { url_path, .. } => assert_eq!(url_path, "/"),
            other => panic!("expected a listing, got {other:?}"),
        }
    }

    #[test]
    fn serves_index_html_for_a_directory_that_has_one() {
        let tree = TempTree::new("index");
        tree.dir("sub");
        tree.file("sub/index.html", "<h1>sub index</h1>");
        assert_eq!(
            resolve_target(tree.path(), "/sub"),
            Target::File(tree.path().join("sub").join("index.html"))
        );
    }

    #[test]
    fn misses_are_not_found() {
        let tree = TempTree::new("missing");
        assert_eq!(resolve_target(tree.path(), "/nope.txt"), Target::NotFound);
    }

    #[test]
    fn does_not_leak_files_outside_the_root_via_traversal() {
        let tree = TempTree::new("traversal");
        tree.file("hello.txt", "hi");
        // `..` is collapsed before resolution, so this stays inside the root
        // and simply misses — it never reaches /etc/passwd.
        assert_eq!(
            resolve_target(tree.path(), "/../../etc/passwd"),
            Target::NotFound
        );
        assert_eq!(
            resolve_target(tree.path(), "/%2e%2e/%2e%2e/etc/passwd"),
            Target::NotFound
        );
    }

    #[cfg(windows)]
    #[test]
    fn refuses_an_absolute_windows_path() {
        let tree = TempTree::new("drive");
        // `push`ing a drive-qualified segment would otherwise escape the root.
        assert_eq!(
            resolve_target(tree.path(), "/C:/Windows/System32/drivers/etc/hosts"),
            Target::Forbidden
        );
    }

    #[test]
    fn sorts_directories_first_then_alphabetically() {
        let tree = TempTree::new("sort");
        tree.file("b.txt", "");
        tree.file("a.txt", "");
        tree.dir("zdir");
        let entries = read_entries(tree.path());
        assert_eq!(entries[0].name, "zdir");
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].name, "a.txt");
        assert_eq!(entries[2].name, "b.txt");
    }

    #[test]
    fn the_listing_links_every_entry() {
        let entries = vec![
            Entry {
                name: "sub".into(),
                is_dir: true,
            },
            Entry {
                name: "hello.txt".into(),
                is_dir: false,
            },
        ];
        let html = directory_listing_html(&entries, "/");
        assert!(html.contains("Directory listing for /"));
        assert!(html.contains("href=\"/sub/\">sub/</a>"));
        assert!(html.contains("href=\"/hello.txt\">hello.txt</a>"));
        // The root has no parent link.
        assert!(!html.contains("../"));
    }

    #[test]
    fn subdirectory_listings_link_back_up() {
        let html = directory_listing_html(&[], "/sub");
        assert!(html.contains("href=\"/sub/..\">../</a>"));
    }

    #[test]
    fn the_listing_escapes_html_in_file_names() {
        let entries = vec![Entry {
            name: "<script>.txt".into(),
            is_dir: false,
        }];
        let html = directory_listing_html(&entries, "/");
        assert!(html.contains("&lt;script&gt;.txt"));
        assert!(!html.contains("<script>.txt"));
    }

    #[test]
    fn the_listing_encodes_spaces_in_links() {
        let entries = vec![Entry {
            name: "my file.txt".into(),
            is_dir: false,
        }];
        let html = directory_listing_html(&entries, "/");
        assert!(html.contains("href=\"/my%20file.txt\""));
    }

    #[test]
    fn types_common_files_by_extension() {
        assert_eq!(
            mime_for(Path::new("a/index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            mime_for(Path::new("a/app.JS")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(mime_for(Path::new("a/logo.png")), "image/png");
        assert_eq!(
            mime_for(Path::new("a/data.bin")),
            "application/octet-stream"
        );
        assert_eq!(mime_for(Path::new("a/noext")), "application/octet-stream");
    }
}
