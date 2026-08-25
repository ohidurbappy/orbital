//! `orbital ftp` — share the current directory over anonymous FTP.
//!
//! Any username gets in, there is no password, and the server is read-only
//! unless started with `--write`. Handy for devices and apps that speak FTP
//! but not HTTP: file managers, TVs, media players, microcontrollers.

pub mod proto;
pub mod server;

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use proto::{ftp_urls, parse_ftp_args, FtpArgs};
use server::{FtpConfig, FtpServer};

use crate::commands::ip::addresses::{local_ips, local_ipv4};
use crate::commands::{Command, Ctx, Stdin};
use crate::style;
use crate::term::{self, Key, RawMode};
use crate::Res;

pub const COMMAND: Command = Command {
    name: "ftp",
    description: "Serve the current directory over FTP (e.g. orbital ftp --write)",
    aliases: &["ftpd"],
    run: None,
    view,
    stdin: Stdin::Never,
};

fn view(ctx: &Ctx) -> Res {
    let options = parse_ftp_args(ctx.args);
    let root = std::env::current_dir()?;

    let server = match FtpServer::bind(FtpConfig {
        root: root.clone(),
        bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        port: options.port,
        writable: options.writable,
    }) {
        Ok(server) => server,
        Err(err) => {
            term::emit(&[style::red(&format!(
                "Could not start FTP on port {}: {err}",
                options.port
            ))]);
            return Ok(());
        }
    };

    term::emit(&render(&root, server.port(), options, ctx.interactive));

    // Serve on a worker thread so this one stays free to watch for Esc.
    let stop = server.stop_handle();
    let worker = std::thread::spawn(move || server.run());

    if ctx.interactive {
        let _raw = RawMode::enable()?;
        loop {
            match term::read_key()? {
                Key::Escape | Key::Interrupt => break,
                _ => {}
            }
        }
        // Cuts the accept loop and every live session/transfer loose, so
        // nothing keeps serving behind the menu's back.
        stop.stop();
    }
    // Not interactive (piped output): serve until Ctrl-C kills the process.
    worker.join().ok();
    Ok(())
}

fn render(root: &Path, port: u16, options: FtpArgs, interactive: bool) -> Vec<String> {
    let entries = local_ips();
    let urls = ftp_urls(port, local_ipv4(&entries).map(|e| e.address.as_str()));
    let access = if options.writable {
        "read + write"
    } else {
        "read-only"
    };

    let mut lines = vec![
        format!(
            "{}{} ({access})",
            style::bold_green("Sharing "),
            root.display()
        ),
        String::new(),
        format!(
            "{}{}",
            style::dim(&style::pad("Local", 10)),
            style::cyan(&urls.local)
        ),
    ];
    if let Some(network) = &urls.network {
        lines.push(format!(
            "{}{}",
            style::dim(&style::pad("Network", 10)),
            style::cyan(network)
        ));
    }

    lines.push(String::new());
    lines.push(style::yellow(
        "Anonymous plain FTP: no password, no encryption — anyone on the network can connect.",
    ));
    if !options.writable {
        lines.push(style::dim(
            "Uploads are off; run with --write to allow them.",
        ));
    }
    lines.push(String::new());
    lines.push(style::dim(if interactive {
        "Press Esc to stop."
    } else {
        "Press Ctrl-C to stop."
    }));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(writable: bool) -> FtpArgs {
        FtpArgs {
            port: 2121,
            writable,
        }
    }

    #[test]
    fn announces_the_shared_directory_and_urls() {
        let out = render(Path::new("/tmp/share"), 2121, options(false), true).join("\n");
        assert!(out.contains("Sharing "));
        assert!(out.contains("ftp://localhost:2121"));
    }

    #[test]
    fn says_which_access_mode_is_live() {
        let read_only = render(Path::new("."), 2121, options(false), true).join("\n");
        assert!(read_only.contains("(read-only)"));
        assert!(read_only.contains("--write"));

        let writable = render(Path::new("."), 2121, options(true), true).join("\n");
        assert!(writable.contains("(read + write)"));
        assert!(!writable.contains("Uploads are off"));
    }

    #[test]
    fn warns_that_the_share_is_open_to_the_network() {
        let out = render(Path::new("."), 2121, options(false), true).join("\n");
        assert!(out.contains("no password"));
        assert!(out.contains("anyone on the network"));
    }

    #[test]
    fn tells_the_user_how_to_stop_it() {
        let interactive = render(Path::new("."), 2121, options(false), true).join("\n");
        assert!(interactive.contains("Press Esc to stop."));
        let piped = render(Path::new("."), 2121, options(false), false).join("\n");
        assert!(piped.contains("Press Ctrl-C to stop."));
    }
}
