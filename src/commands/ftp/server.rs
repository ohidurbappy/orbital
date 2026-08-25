//! The socket side of `orbital ftp` — the listener, one thread per control
//! connection, and the data connections transfers run over. Every protocol
//! decision (parsing, formatting, path jailing) is a pure function in `proto`;
//! this module only moves bytes.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::proto::{
    epsv_reply, feat_lines, list_line, mkd_reply, mlsd_line, nlst_line, parse_command, parse_eprt,
    parse_host_port, pasv_reply, pwd_reply, real_path, resolve_virtual, strip_list_flags,
    strip_telnet, virtual_display, EntryMeta,
};

/// How long a passive listener waits for the client to dial in, and how long
/// an active connect-back may take. Generous — LAN clients answer in
/// milliseconds — but bounded, so a vanished client can't park a thread.
const DATA_TIMEOUT: Duration = Duration::from_secs(15);

/// Control lines longer than this are rejected, so a hostile peer can't grow
/// the read buffer without bound.
const MAX_LINE: u64 = 4096;

pub struct FtpConfig {
    pub root: PathBuf,
    pub bind: IpAddr,
    pub port: u16,
    pub writable: bool,
}

/// Every live socket, so stopping the server can cut sessions and in-flight
/// transfers loose instead of leaving them running behind the user's back.
struct Registry {
    stopped: AtomicBool,
    next_id: AtomicU64,
    sockets: Mutex<HashMap<u64, TcpStream>>,
}

impl Registry {
    fn new() -> Self {
        Self {
            stopped: AtomicBool::new(false),
            next_id: AtomicU64::new(0),
            sockets: Mutex::new(HashMap::new()),
        }
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Register a live socket so `stop` can shut it down. On a server that has
    /// already stopped the socket is shut down immediately instead.
    fn track(&self, stream: &TcpStream) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        if let (Ok(clone), Ok(mut sockets)) = (stream.try_clone(), self.sockets.lock()) {
            sockets.insert(id, clone);
        }
        // Checked after inserting: `stop` may have drained the map in between,
        // and this way the socket still gets shut down rather than lingering.
        if self.is_stopped() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        id
    }

    fn untrack(&self, id: u64) {
        if let Ok(mut sockets) = self.sockets.lock() {
            sockets.remove(&id);
        }
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Ok(mut sockets) = self.sockets.lock() {
            for (_, socket) in sockets.drain() {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
    }
}

/// Stops a running [`FtpServer`] from another thread: flips the flag, pokes
/// the blocking accept awake, and shuts down every live session socket.
pub struct StopHandle {
    registry: Arc<Registry>,
    wake: SocketAddr,
}

impl StopHandle {
    pub fn stop(&self) {
        self.registry.stop();
        // Poke the blocking accept so the loop observes the flag.
        let _ = TcpStream::connect_timeout(&self.wake, Duration::from_millis(250));
    }
}

pub struct FtpServer {
    listener: TcpListener,
    config: Arc<FtpConfig>,
    registry: Arc<Registry>,
}

impl FtpServer {
    pub fn bind(config: FtpConfig) -> io::Result<Self> {
        let listener = TcpListener::bind((config.bind, config.port))?;
        Ok(Self {
            listener,
            config: Arc::new(config),
            registry: Arc::new(Registry::new()),
        })
    }

    /// The port actually bound — differs from the config when it asked for 0.
    pub fn port(&self) -> u16 {
        self.listener
            .local_addr()
            .map(|addr| addr.port())
            .unwrap_or(self.config.port)
    }

    pub fn stop_handle(&self) -> StopHandle {
        let ip = match self.config.bind {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        StopHandle {
            registry: Arc::clone(&self.registry),
            wake: SocketAddr::new(ip, self.port()),
        }
    }

    /// Accept until stopped; each connection gets its own session thread.
    pub fn run(self) {
        for conn in self.listener.incoming() {
            if self.registry.is_stopped() {
                break;
            }
            let Ok(stream) = conn else { continue };
            let config = Arc::clone(&self.config);
            let registry = Arc::clone(&self.registry);
            std::thread::spawn(move || {
                let _ = session(stream, &config, &registry);
            });
        }
    }
}

fn session(stream: TcpStream, config: &FtpConfig, registry: &Registry) -> io::Result<()> {
    let id = registry.track(&stream);
    let result = run_session(stream, config, registry);
    registry.untrack(id);
    result
}

/// One control line, bounded by [`MAX_LINE`].
enum Line {
    Eof,
    TooLong,
    Cmd(String),
}

fn read_command(reader: &mut BufReader<TcpStream>) -> io::Result<Line> {
    let mut buf = Vec::new();
    let n = reader.by_ref().take(MAX_LINE).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(Line::Eof);
    }
    if !buf.ends_with(b"\n") && n == MAX_LINE as usize {
        // Oversized: throw away the rest of the line, then complain once.
        loop {
            let mut rest = Vec::new();
            let m = reader
                .by_ref()
                .take(MAX_LINE)
                .read_until(b'\n', &mut rest)?;
            if m == 0 || rest.ends_with(b"\n") {
                break;
            }
        }
        return Ok(Line::TooLong);
    }
    let text = String::from_utf8_lossy(strip_telnet(&buf)).into_owned();
    Ok(Line::Cmd(text))
}

fn run_session(control: TcpStream, config: &FtpConfig, registry: &Registry) -> io::Result<()> {
    let mut reader = BufReader::new(control.try_clone()?);
    let mut session = Session {
        config,
        registry,
        control,
        cwd: Vec::new(),
        data: DataMode::None,
        rest: 0,
        rename_from: None,
    };
    session.reply("220 orbital ftp ready.")?;

    loop {
        match read_command(&mut reader)? {
            Line::Eof => return Ok(()),
            Line::TooLong => session.reply("500 Line too long.")?,
            Line::Cmd(text) => {
                let (verb, arg) = parse_command(&text);
                if verb.is_empty() {
                    session.reply("500 Empty command.")?;
                    continue;
                }
                if verb == "QUIT" {
                    return session.reply("221 Bye.");
                }
                session.dispatch(&verb, &arg)?;
            }
        }
    }
}

/// How the next transfer will get its data connection.
enum DataMode {
    None,
    /// PASV/EPSV: we listen, the client dials in.
    Passive(TcpListener),
    /// PORT/EPRT: the client listens, we dial out.
    Active(SocketAddr),
}

struct Session<'a> {
    config: &'a FtpConfig,
    registry: &'a Registry,
    control: TcpStream,
    /// Virtual working directory, as jailed path components.
    cwd: Vec<String>,
    data: DataMode,
    /// Offset set by REST, consumed by the next RETR.
    rest: u64,
    /// Source path staged by RNFR, consumed by RNTO.
    rename_from: Option<PathBuf>,
}

impl Session<'_> {
    fn reply(&mut self, text: &str) -> io::Result<()> {
        self.control.write_all(text.as_bytes())?;
        self.control.write_all(b"\r\n")
    }

    fn reply_lines(&mut self, lines: &[String]) -> io::Result<()> {
        for line in lines {
            self.reply(line)?;
        }
        Ok(())
    }

    /// Resolve a client path against the cwd into (virtual, real), or `None`
    /// when it can't name anything inside the shared root.
    fn locate(&self, arg: &str) -> Option<(Vec<String>, PathBuf)> {
        let parts = resolve_virtual(&self.cwd, arg)?;
        let real = real_path(&self.config.root, &parts)?;
        Some((parts, real))
    }

    fn dispatch(&mut self, verb: &str, arg: &str) -> io::Result<()> {
        // Every mutating verb is refused up front on a read-only server.
        if !self.config.writable
            && matches!(
                verb,
                "STOR" | "APPE" | "DELE" | "MKD" | "XMKD" | "RMD" | "XRMD" | "RNFR" | "RNTO"
            )
        {
            return self.reply("550 Read-only server; restart with --write to allow changes.");
        }

        match verb {
            // Anonymous by design: any USER is let straight in, and a PASS
            // sent anyway is fine too.
            "USER" => self.reply("230 Anonymous access granted."),
            "PASS" => self.reply("230 Already logged in."),
            "SYST" => self.reply("215 UNIX Type: L8"),
            "FEAT" => self.reply_lines(&feat_lines()),
            "OPTS" => {
                let option = arg.to_ascii_uppercase();
                if option == "UTF8 ON" || option.starts_with("MLST") {
                    self.reply("200 OK.")
                } else {
                    self.reply("501 Unsupported option.")
                }
            }
            "NOOP" => self.reply("200 OK."),
            "HELP" => self.reply("214 Anonymous FTP server; standard commands supported."),
            // Transfers are always the raw bytes, so every TYPE is "fine".
            "TYPE" => self.reply("200 OK; data is sent as-is (binary)."),
            "MODE" => {
                if arg.eq_ignore_ascii_case("S") {
                    self.reply("200 OK.")
                } else {
                    self.reply("504 Only stream mode.")
                }
            }
            "STRU" => {
                if arg.eq_ignore_ascii_case("F") {
                    self.reply("200 OK.")
                } else {
                    self.reply("504 Only file structure.")
                }
            }
            "PWD" | "XPWD" => {
                let reply = pwd_reply(&self.cwd);
                self.reply(&reply)
            }
            "CWD" | "XCWD" => self.cmd_cwd(arg),
            "CDUP" | "XCUP" => {
                self.cwd.pop();
                self.reply("250 OK.")
            }
            "PASV" => self.cmd_pasv(),
            "EPSV" => self.cmd_epsv(arg),
            "PORT" => match parse_host_port(arg) {
                Some((ip, port)) => self.set_active(IpAddr::V4(ip), port),
                None => self.reply("501 Bad PORT argument."),
            },
            "EPRT" => match parse_eprt(arg) {
                Some((ip, port)) => self.set_active(ip, port),
                None => self.reply("501 Bad EPRT argument."),
            },
            "LIST" | "NLST" | "MLSD" => self.cmd_list(verb, arg),
            "MLST" => self.cmd_mlst(arg),
            "RETR" => self.cmd_retr(arg),
            "STOR" => self.cmd_stor(arg, false),
            "APPE" => self.cmd_stor(arg, true),
            "REST" => match arg.parse::<u64>() {
                Ok(offset) => {
                    self.rest = offset;
                    let reply = format!("350 Restarting at {offset}.");
                    self.reply(&reply)
                }
                Err(_) => self.reply("501 Bad restart offset."),
            },
            "SIZE" => self.cmd_size(arg),
            "MDTM" => self.cmd_mdtm(arg),
            "DELE" => self.cmd_dele(arg),
            "MKD" | "XMKD" => self.cmd_mkd(arg),
            "RMD" | "XRMD" => self.cmd_rmd(arg),
            "RNFR" => self.cmd_rnfr(arg),
            "RNTO" => self.cmd_rnto(arg),
            // Transfers are synchronous, so by the time an ABOR is read there
            // is nothing left to abort.
            "ABOR" => {
                self.data = DataMode::None;
                self.reply("226 Nothing to abort.")
            }
            _ => self.reply("502 Command not implemented."),
        }
    }

    fn cmd_cwd(&mut self, arg: &str) -> io::Result<()> {
        let Some((parts, real)) = self.locate(arg) else {
            return self.reply("550 No such directory.");
        };
        if fs::metadata(&real).map(|m| m.is_dir()).unwrap_or(false) {
            self.cwd = parts;
            self.reply("250 Directory changed.")
        } else {
            self.reply("550 No such directory.")
        }
    }

    /// Bind a fresh passive listener on the control connection's own address,
    /// so the advertised endpoint is the one the client already reaches us on.
    fn open_passive(&mut self) -> io::Result<Option<u16>> {
        let local_ip = self.control.local_addr()?.ip();
        let listener = match TcpListener::bind((local_ip, 0)) {
            Ok(listener) => listener,
            Err(_) => return Ok(None),
        };
        let port = listener.local_addr()?.port();
        self.data = DataMode::Passive(listener);
        Ok(Some(port))
    }

    fn cmd_pasv(&mut self) -> io::Result<()> {
        let ip = match self.control.local_addr()?.ip() {
            IpAddr::V4(ip) => ip,
            IpAddr::V6(_) => return self.reply("425 PASV needs IPv4; use EPSV."),
        };
        match self.open_passive()? {
            Some(port) => {
                let reply = pasv_reply(ip, port);
                self.reply(&reply)
            }
            None => self.reply("425 Can't open a data port."),
        }
    }

    fn cmd_epsv(&mut self, arg: &str) -> io::Result<()> {
        // "EPSV ALL" just asks us to expect EPSV from now on.
        if arg.eq_ignore_ascii_case("ALL") {
            return self.reply("200 OK.");
        }
        match self.open_passive()? {
            Some(port) => {
                let reply = epsv_reply(port);
                self.reply(&reply)
            }
            None => self.reply("425 Can't open a data port."),
        }
    }

    /// Accept a PORT/EPRT target — but only pointing back at the machine that
    /// owns this session, and never at a privileged port. Anything else is the
    /// classic FTP bounce attack.
    fn set_active(&mut self, ip: IpAddr, port: u16) -> io::Result<()> {
        let peer = self.control.peer_addr()?.ip();
        if ip != peer {
            return self.reply("501 Data address must match your own address.");
        }
        if port < 1024 {
            return self.reply("501 Data port too low.");
        }
        self.data = DataMode::Active(SocketAddr::new(ip, port));
        self.reply("200 OK.")
    }

    /// Open the data connection the client prepared. Consumes the prepared
    /// mode: FTP data connections are single-use.
    fn open_data(&mut self) -> Option<TcpStream> {
        let peer_ip = self.control.peer_addr().ok()?.ip();
        match std::mem::replace(&mut self.data, DataMode::None) {
            DataMode::None => None,
            DataMode::Passive(listener) => accept_from(listener, peer_ip, DATA_TIMEOUT),
            DataMode::Active(addr) => TcpStream::connect_timeout(&addr, DATA_TIMEOUT).ok(),
        }
    }

    /// Run one transfer over the data connection: open, `150`, body, close,
    /// then `226` or `426`. The stream is registered so a server stop cuts
    /// in-flight transfers loose too.
    fn transfer(&mut self, body: impl FnOnce(&mut TcpStream) -> io::Result<()>) -> io::Result<()> {
        let Some(mut stream) = self.open_data() else {
            return self.reply("425 No data connection; send PASV or PORT first.");
        };
        self.reply("150 Opening data connection.")?;
        let id = self.registry.track(&stream);
        let outcome = body(&mut stream);
        let _ = stream.shutdown(Shutdown::Both);
        self.registry.untrack(id);
        drop(stream);
        match outcome {
            Ok(()) => self.reply("226 Transfer complete."),
            Err(_) => self.reply("426 Transfer failed."),
        }
    }

    fn cmd_list(&mut self, verb: &str, arg: &str) -> io::Result<()> {
        let path_arg = if verb == "MLSD" {
            arg
        } else {
            strip_list_flags(arg)
        };
        let Some((_, real)) = self.locate(path_arg) else {
            return self.reply("550 Not found.");
        };
        let Ok(metadata) = fs::metadata(&real) else {
            return self.reply("550 Not found.");
        };

        let entries = if metadata.is_dir() {
            collect_entries(&real)
        } else if verb == "MLSD" {
            return self.reply("501 MLSD needs a directory.");
        } else {
            // `LIST file.txt` — a listing of just that file.
            let name = real
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path_arg.to_string());
            vec![entry_meta(name, &metadata)]
        };

        let now = now_secs();
        let writable = self.config.writable;
        let lines: Vec<String> = entries
            .iter()
            .map(|entry| match verb {
                "NLST" => nlst_line(entry),
                "MLSD" => mlsd_line(entry),
                _ => list_line(entry, now, writable),
            })
            .collect();

        self.transfer(move |data| {
            for line in &lines {
                data.write_all(line.as_bytes())?;
                data.write_all(b"\r\n")?;
            }
            Ok(())
        })
    }

    fn cmd_mlst(&mut self, arg: &str) -> io::Result<()> {
        let Some((parts, real)) = self.locate(arg) else {
            return self.reply("550 Not found.");
        };
        let Ok(metadata) = fs::metadata(&real) else {
            return self.reply("550 Not found.");
        };
        let entry = entry_meta(virtual_display(&parts), &metadata);
        self.reply_lines(&[
            "250-Listing".to_string(),
            format!(" {}", mlsd_line(&entry)),
            "250 End.".to_string(),
        ])
    }

    fn cmd_retr(&mut self, arg: &str) -> io::Result<()> {
        let Some((_, real)) = self.locate(arg) else {
            return self.reply("550 Not found.");
        };
        // On Unix, opening a directory "succeeds" — the metadata check keeps
        // RETR strictly for files on every platform.
        let mut file = match File::open(&real) {
            Ok(file) if file.metadata().map(|m| m.is_file()).unwrap_or(false) => file,
            _ => return self.reply("550 Not found."),
        };
        let offset = std::mem::take(&mut self.rest);
        if offset > 0 && file.seek(SeekFrom::Start(offset)).is_err() {
            return self.reply("550 Bad restart offset.");
        }
        self.transfer(move |data| io::copy(&mut file, data).map(|_| ()))
    }

    fn cmd_stor(&mut self, arg: &str, append: bool) -> io::Result<()> {
        self.rest = 0;
        let Some((_, real)) = self.locate(arg) else {
            return self.reply("550 Bad path.");
        };
        let mut options = OpenOptions::new();
        options.write(true).create(true);
        if append {
            options.append(true);
        } else {
            options.truncate(true);
        }
        let mut file = match options.open(&real) {
            Ok(file) => file,
            Err(_) => return self.reply("550 Can't write there."),
        };
        self.transfer(move |data| io::copy(data, &mut file).map(|_| ()))
    }

    fn cmd_size(&mut self, arg: &str) -> io::Result<()> {
        match self
            .locate(arg)
            .and_then(|(_, real)| fs::metadata(real).ok())
        {
            Some(metadata) if metadata.is_file() => {
                let reply = format!("213 {}", metadata.len());
                self.reply(&reply)
            }
            _ => self.reply("550 Not a file."),
        }
    }

    fn cmd_mdtm(&mut self, arg: &str) -> io::Result<()> {
        match self
            .locate(arg)
            .and_then(|(_, real)| fs::metadata(real).ok())
        {
            Some(metadata) => {
                let reply = format!("213 {}", super::proto::format_mdtm(mtime_secs(&metadata)));
                self.reply(&reply)
            }
            None => self.reply("550 Not found."),
        }
    }

    fn cmd_dele(&mut self, arg: &str) -> io::Result<()> {
        let deleted = self
            .locate(arg)
            .is_some_and(|(_, real)| fs::remove_file(real).is_ok());
        if deleted {
            self.reply("250 Deleted.")
        } else {
            self.reply("550 Can't delete that.")
        }
    }

    fn cmd_mkd(&mut self, arg: &str) -> io::Result<()> {
        if let Some((parts, real)) = self.locate(arg) {
            if fs::create_dir(real).is_ok() {
                let reply = mkd_reply(&parts);
                return self.reply(&reply);
            }
        }
        self.reply("550 Can't create that directory.")
    }

    fn cmd_rmd(&mut self, arg: &str) -> io::Result<()> {
        // remove_dir is deliberately non-recursive: RMD of a full directory
        // fails instead of silently wiping a tree.
        let removed = self
            .locate(arg)
            .is_some_and(|(_, real)| fs::remove_dir(real).is_ok());
        if removed {
            self.reply("250 Removed.")
        } else {
            self.reply("550 Can't remove that directory.")
        }
    }

    fn cmd_rnfr(&mut self, arg: &str) -> io::Result<()> {
        match self.locate(arg) {
            Some((_, real)) if fs::symlink_metadata(&real).is_ok() => {
                self.rename_from = Some(real);
                self.reply("350 Ready; send RNTO.")
            }
            _ => self.reply("550 Not found."),
        }
    }

    fn cmd_rnto(&mut self, arg: &str) -> io::Result<()> {
        let Some(from) = self.rename_from.take() else {
            return self.reply("503 Send RNFR first.");
        };
        let renamed = self
            .locate(arg)
            .is_some_and(|(_, to)| fs::rename(&from, to).is_ok());
        if renamed {
            self.reply("250 Renamed.")
        } else {
            self.reply("550 Can't rename to that.")
        }
    }
}

/// Accept the client's data connection, refusing strangers: only the control
/// connection's peer may attach (the passive half of the bounce defence).
fn accept_from(listener: TcpListener, want: IpAddr, timeout: Duration) -> Option<TcpStream> {
    if listener.set_nonblocking(true).is_err() {
        return None;
    }
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, peer)) => {
                if peer.ip() == want {
                    // Accepted sockets inherit non-blocking on some platforms.
                    if stream.set_nonblocking(false).is_err() {
                        return None;
                    }
                    return Some(stream);
                }
                let _ = stream.shutdown(Shutdown::Both);
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

fn mtime_secs(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn entry_meta(name: String, metadata: &fs::Metadata) -> EntryMeta {
    EntryMeta {
        name,
        is_dir: metadata.is_dir(),
        size: metadata.len(),
        mtime_secs: mtime_secs(metadata),
    }
}

/// Read a directory into listing entries: directories first, then
/// alphabetical — the same order `orbital serve` lists.
fn collect_entries(dir: &Path) -> Vec<EntryMeta> {
    let mut entries: Vec<EntryMeta> = match fs::read_dir(dir) {
        Ok(reader) => reader
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                Some(entry_meta(
                    entry.file_name().to_string_lossy().into_owned(),
                    &metadata,
                ))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    entries
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway directory tree for each test server.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("orbital-ftp-{}-{tag}", std::process::id()));
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

    fn start(tag: &str, writable: bool) -> (TempTree, u16, StopHandle) {
        let tree = TempTree::new(tag);
        let server = FtpServer::bind(FtpConfig {
            root: tree.path().to_path_buf(),
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            writable,
        })
        .unwrap();
        let port = server.port();
        let stop = server.stop_handle();
        std::thread::spawn(move || server.run());
        (tree, port, stop)
    }

    struct Client {
        stream: TcpStream,
        reader: BufReader<TcpStream>,
    }

    impl Client {
        fn connect(port: u16) -> Self {
            let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let reader = BufReader::new(stream.try_clone().unwrap());
            let mut client = Self { stream, reader };
            let greeting = client.read_reply();
            assert!(greeting.starts_with("220"), "greeting: {greeting}");
            client
        }

        fn read_reply(&mut self) -> String {
            let mut line = String::new();
            self.reader.read_line(&mut line).unwrap();
            line.trim_end().to_string()
        }

        /// Send a command, read its single-line reply.
        fn cmd(&mut self, command: &str) -> String {
            self.send(command);
            self.read_reply()
        }

        fn send(&mut self, command: &str) {
            self.stream
                .write_all(format!("{command}\r\n").as_bytes())
                .unwrap();
        }

        /// EPSV, then dial the advertised data port.
        fn open_passive_data(&mut self) -> TcpStream {
            let reply = self.cmd("EPSV");
            assert!(reply.starts_with("229"), "{reply}");
            let digits: String = reply
                .split("|||")
                .nth(1)
                .unwrap()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let data = TcpStream::connect(("127.0.0.1", digits.parse::<u16>().unwrap())).unwrap();
            data.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            data
        }

        /// Run a download-style command over a passive data connection,
        /// returning (opening reply, data bytes, closing reply).
        fn download(&mut self, command: &str) -> (String, Vec<u8>, String) {
            let mut data = self.open_passive_data();
            let opening = self.cmd(command);
            let mut body = Vec::new();
            data.read_to_end(&mut body).unwrap();
            drop(data);
            let closing = self.read_reply();
            (opening, body, closing)
        }
    }

    #[test]
    fn greets_logs_anyone_in_and_navigates() {
        let (tree, port, _stop) = start("nav", false);
        tree.dir("sub");
        let mut c = Client::connect(port);
        assert!(c.cmd("USER anonymous").starts_with("230"));
        assert!(c.cmd("PASS whatever").starts_with("230"));
        assert!(c.cmd("SYST").starts_with("215"));
        assert_eq!(c.cmd("PWD"), "257 \"/\" is the current directory.");
        assert!(c.cmd("CWD sub").starts_with("250"));
        assert!(c.cmd("PWD").contains("\"/sub\""));
        assert!(c.cmd("CDUP").starts_with("250"));
        assert!(c.cmd("CWD nope").starts_with("550"));
        assert!(c.cmd("WHAT").starts_with("502"));
        assert!(c.cmd("QUIT").starts_with("221"));
    }

    #[test]
    fn lists_a_directory_over_passive_data() {
        let (tree, port, _stop) = start("list", false);
        tree.file("hello.txt", "hi there"); // 8 bytes
        tree.dir("sub");
        let mut c = Client::connect(port);
        let (opening, body, closing) = c.download("LIST");
        assert!(opening.starts_with("150"), "{opening}");
        assert!(closing.starts_with("226"), "{closing}");
        let listing = String::from_utf8(body).unwrap();
        let file_line = listing
            .lines()
            .find(|l| l.ends_with("hello.txt"))
            .expect("hello.txt listed");
        assert!(file_line.starts_with("-r--"), "{file_line}");
        assert!(file_line.contains(" 8 "), "{file_line}");
        assert!(listing
            .lines()
            .any(|l| l.starts_with('d') && l.ends_with("sub")));
    }

    #[test]
    fn serves_machine_readable_listings() {
        let (tree, port, _stop) = start("mlsd", false);
        tree.file("a.txt", "12345");
        let mut c = Client::connect(port);
        let (_, body, _) = c.download("MLSD");
        let listing = String::from_utf8(body).unwrap();
        assert!(listing.contains("type=file;size=5;modify="), "{listing}");
        assert!(listing.trim_end().ends_with("; a.txt"), "{listing}");
        let (_, names, _) = c.download("NLST");
        assert_eq!(String::from_utf8(names).unwrap().trim(), "a.txt");
    }

    #[test]
    fn retrieves_files_and_resumes_with_rest() {
        let (tree, port, _stop) = start("retr", false);
        tree.file("song.txt", "abcdefgh");
        let mut c = Client::connect(port);
        let (_, body, closing) = c.download("RETR song.txt");
        assert_eq!(body, b"abcdefgh");
        assert!(closing.starts_with("226"));

        assert!(c.cmd("REST 6").starts_with("350"));
        let (_, tail, _) = c.download("RETR song.txt");
        assert_eq!(tail, b"gh");

        // A miss answers on the control channel without touching data.
        let _spare = c.open_passive_data();
        assert!(c.cmd("RETR missing.txt").starts_with("550"));
    }

    #[test]
    fn read_only_mode_refuses_every_write() {
        let (tree, port, _stop) = start("ro", false);
        tree.file("keep.txt", "safe");
        let mut c = Client::connect(port);
        for refused in [
            "STOR up.txt",
            "APPE up.txt",
            "DELE keep.txt",
            "MKD box",
            "RMD box",
            "RNFR keep.txt",
        ] {
            let reply = c.cmd(refused);
            assert!(
                reply.starts_with("550") && reply.contains("--write"),
                "{refused}: {reply}"
            );
        }
        assert!(tree.path().join("keep.txt").exists());
    }

    #[test]
    fn write_mode_uploads_renames_and_deletes() {
        let (tree, port, _stop) = start("rw", true);
        let mut c = Client::connect(port);

        let mut data = c.open_passive_data();
        assert!(c.cmd("STOR up.txt").starts_with("150"));
        data.write_all(b"uploaded").unwrap();
        drop(data); // FIN ends the transfer
        assert!(c.read_reply().starts_with("226"));
        assert_eq!(
            fs::read_to_string(tree.path().join("up.txt")).unwrap(),
            "uploaded"
        );

        assert_eq!(c.cmd("MKD box"), "257 \"/box\" created.");
        assert!(tree.path().join("box").is_dir());
        assert!(c.cmd("RNFR up.txt").starts_with("350"));
        assert!(c.cmd("RNTO box/up2.txt").starts_with("250"));
        assert!(tree.path().join("box").join("up2.txt").exists());
        assert!(c.cmd("RNTO nowhere.txt").starts_with("503"));
        assert!(c.cmd("DELE box/up2.txt").starts_with("250"));
        assert!(c.cmd("RMD box").starts_with("250"));
        assert!(!tree.path().join("box").exists());
    }

    #[test]
    fn jails_every_path_inside_the_root() {
        let (tree, port, _stop) = start("jail", false);
        tree.file("inside.txt", "ok");
        // A secret directly beside the shared root.
        let secret =
            std::env::temp_dir().join(format!("orbital-ftp-secret-{}.txt", std::process::id()));
        fs::write(&secret, "secret").unwrap();

        let mut c = Client::connect(port);
        assert!(c.cmd("CWD ..").starts_with("250")); // clamped…
        assert!(c.cmd("PWD").contains("\"/\"")); // …at the root
        let _spare = c.open_passive_data();
        let name = secret.file_name().unwrap().to_str().unwrap();
        assert!(c.cmd(&format!("RETR ../{name}")).starts_with("550"));
        assert!(c.cmd(&format!("RETR ..\\..\\{name}")).starts_with("550"));
        assert!(c.cmd("RETR C:\\evil.txt").starts_with("550"));
        assert!(c.cmd("SIZE inside.txt").starts_with("213")); // sanity: still serving

        let _ = fs::remove_file(secret);
    }

    #[test]
    fn refuses_third_party_data_targets() {
        let (_tree, port, _stop) = start("bounce", false);
        let mut c = Client::connect(port);
        // Bounce attempts: a stranger's address, a privileged port, garbage.
        assert!(c.cmd("PORT 203,0,113,9,16,97").starts_with("501"));
        assert!(c.cmd("PORT 127,0,0,1,0,80").starts_with("501"));
        assert!(c.cmd("PORT 1,2,3").starts_with("501"));
        assert!(c.cmd("EPRT |1|203.0.113.9|4193|").starts_with("501"));
    }

    #[test]
    fn active_mode_connects_back_to_the_client() {
        let (tree, port, _stop) = start("active", false);
        tree.file("a.txt", "abc");
        let mut c = Client::connect(port);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let data_port = listener.local_addr().unwrap().port();
        let reply = c.cmd(&format!(
            "PORT 127,0,0,1,{},{}",
            data_port >> 8,
            data_port & 0xff
        ));
        assert!(reply.starts_with("200"), "{reply}");

        c.send("NLST");
        let (mut data, _) = listener.accept().unwrap();
        data.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut body = String::new();
        data.read_to_string(&mut body).unwrap();
        assert_eq!(body.trim(), "a.txt");
        assert!(c.read_reply().starts_with("150"));
        assert!(c.read_reply().starts_with("226"));
    }

    #[test]
    fn reports_size_mdtm_and_features() {
        let (tree, port, _stop) = start("meta", false);
        tree.file("f.txt", "12345");
        let mut c = Client::connect(port);
        assert_eq!(c.cmd("SIZE f.txt"), "213 5");
        let mdtm = c.cmd("MDTM f.txt");
        assert!(mdtm.starts_with("213 "), "{mdtm}");
        assert_eq!(mdtm.len(), "213 ".len() + 14);
        assert!(mdtm[4..].chars().all(|ch| ch.is_ascii_digit()));
        assert!(c.cmd("SIZE .").starts_with("550")); // directories have no SIZE

        c.send("FEAT");
        let mut lines = Vec::new();
        loop {
            let line = c.read_reply();
            let done = line.starts_with("211 ");
            lines.push(line);
            if done {
                break;
            }
        }
        let feat = lines.join("\n");
        assert!(feat.contains("UTF8") && feat.contains("MLST") && feat.contains("EPSV"));

        // MLST answers facts on the control channel.
        c.send("MLST f.txt");
        assert!(c.read_reply().starts_with("250-"));
        assert!(c.read_reply().contains("type=file;size=5"));
        assert!(c.read_reply().starts_with("250 "));
    }

    #[test]
    fn survives_oversized_command_lines() {
        let (_tree, port, _stop) = start("bigline", false);
        let mut c = Client::connect(port);
        let reply = c.cmd(&"X".repeat(5000));
        assert!(reply.starts_with("500"), "{reply}");
        assert!(c.cmd("NOOP").starts_with("200")); // the session survives
    }

    #[test]
    fn stop_cuts_live_sessions_loose() {
        let (_tree, port, stop) = start("stop", false);
        let mut c = Client::connect(port);
        assert!(c.cmd("NOOP").starts_with("200"));
        stop.stop();
        // The control socket was shut down under the client: EOF or reset.
        let mut line = String::new();
        let n = c.reader.read_line(&mut line).unwrap_or(0);
        assert_eq!(n, 0, "session should be closed after stop, got {line:?}");
    }
}
