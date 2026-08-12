//! `orbital serve` — share the current directory over HTTP.

pub mod files;

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use tiny_http::{Header, Request, Response, Server};

use files::{
    current_serve_urls, directory_listing_html, mime_for, read_entries, resolve_serve_port,
    resolve_target, Target, DEFAULT_PORT,
};

use crate::commands::qr::encode::to_qr_lines;
use crate::commands::{Command, Ctx};
use crate::style;
use crate::term::{self, Key, RawMode};
use crate::Res;

pub const COMMAND: Command = Command {
    name: "serve",
    description: "Serve the current directory over HTTP (e.g. orbital serve 8080)",
    aliases: &["http"],
    run: None,
    view,
    reads_stdin: false,
};

fn view(ctx: &Ctx) -> Res {
    let port = resolve_serve_port(ctx.args, DEFAULT_PORT);
    let root = std::env::current_dir()?;

    let server = match Server::http(("0.0.0.0", port)) {
        Ok(server) => Arc::new(server),
        Err(err) => {
            term::emit(&[style::red(&format!(
                "Could not start server on port {port}: {err}"
            ))]);
            return Ok(());
        }
    };

    term::emit(&render(&root, port, ctx.interactive));

    // Serve on a worker thread so this one stays free to watch for Esc.
    let worker = {
        let server = Arc::clone(&server);
        let root = root.clone();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let _ = respond(&root, request);
            }
        })
    };

    if ctx.interactive {
        let _raw = RawMode::enable()?;
        loop {
            match term::read_key()? {
                Key::Escape | Key::Interrupt => break,
                _ => {}
            }
        }
    } else {
        // No keyboard to listen on (piped output): serve until Ctrl-C kills us.
        worker.join().ok();
        return Ok(());
    }

    // `unblock` wakes `incoming_requests` so the worker can finish.
    server.unblock();
    worker.join().ok();
    Ok(())
}

fn respond(root: &Path, request: Request) -> std::io::Result<()> {
    let url = request.url().to_string();
    match resolve_target(root, &url) {
        Target::Forbidden => request.respond(text_response("Forbidden", 403)),
        Target::NotFound => request.respond(text_response("Not Found", 404)),
        Target::Listing { dir, url_path } => {
            let html = directory_listing_html(&read_entries(&dir), &url_path);
            request.respond(
                Response::from_string(html).with_header(header("text/html; charset=utf-8")),
            )
        }
        Target::File(path) => match File::open(&path) {
            Ok(file) => {
                request.respond(Response::from_file(file).with_header(header(mime_for(&path))))
            }
            // Raced with a delete, or unreadable — treat as missing.
            Err(_) => request.respond(text_response("Not Found", 404)),
        },
    }
}

fn text_response(body: &str, status: u16) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(status)
        .with_header(header("text/plain; charset=utf-8"))
}

fn header(content_type: &str) -> Header {
    // Both sides are static, well-formed header text.
    Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes())
        .expect("valid Content-Type header")
}

fn render(root: &Path, port: u16, interactive: bool) -> Vec<String> {
    let urls = current_serve_urls(port);
    let mut lines = vec![
        format!("{}{}", style::bold_green("Serving "), root.display()),
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

    let qr_target = urls.network.clone().unwrap_or_else(|| urls.local.clone());
    if let Ok(qr) = to_qr_lines(&qr_target) {
        lines.push(String::new());
        lines.push(style::dim(&format!(
            "Scan to open on your phone ({qr_target}):"
        )));
        lines.push(String::new());
        lines.extend(qr);
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

    #[test]
    fn announces_the_served_directory_and_urls() {
        let lines = render(Path::new("/tmp/site"), 8080, true);
        let out = lines.join("\n");
        assert!(out.contains("Serving "));
        assert!(out.contains("http://localhost:8080"));
    }

    #[test]
    fn tells_the_user_how_to_stop_it() {
        let interactive = render(Path::new("."), 8000, true).join("\n");
        assert!(interactive.contains("Press Esc to stop."));
        let piped = render(Path::new("."), 8000, false).join("\n");
        assert!(piped.contains("Press Ctrl-C to stop."));
    }

    #[test]
    fn includes_a_scannable_code_for_the_reachable_url() {
        let lines = render(Path::new("."), 8000, true);
        let out = lines.join("\n");
        assert!(out.contains("Scan to open on your phone"));
        assert!(lines.iter().any(|l| l.contains('█') || l.contains('▀')));
    }
}
