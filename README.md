# orbital

A growable, cross-platform CLI toolbox — an aggregate of small tools that share one
binary, one update mechanism, and one consistent UI. Written in Rust, shipped as a
single dependency-free executable (~15 MB, most of which is the 45M-parameter
language model behind [`orbital do`](#do)).

## Install

**macOS / Linux** — paste into your terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/ohidurbappy/orbital/main/install.sh | sh
```

**Windows** — paste into PowerShell:

```powershell
irm https://raw.githubusercontent.com/ohidurbappy/orbital/main/install.ps1 | iex
```

The installer detects your platform, downloads the latest release, and installs
it as `orbital` on your `PATH`. Override the location with `ORBITAL_INSTALL_DIR`
if you like. Re-run it any time to upgrade (or use `orbital update`).

<details>
<summary>Manual install</summary>

Grab the `.gz` asset for your platform from the
[latest release](https://github.com/ohidurbappy/orbital/releases/latest), decompress
it, mark it executable, and put it on your `PATH`:

```sh
# example: macOS arm64
curl -fsSL https://github.com/ohidurbappy/orbital/releases/latest/download/orbital-darwin-arm64.gz | gunzip > orbital
chmod +x orbital
sudo mv orbital /usr/local/bin/orbital
```

| Platform      | Asset                        |
| ------------- | ---------------------------- |
| macOS (Apple) | `orbital-darwin-arm64.gz`    |
| macOS (Intel) | `orbital-darwin-x64.gz`      |
| Linux x64     | `orbital-linux-x64.gz`       |
| Linux arm64   | `orbital-linux-arm64.gz`     |
| Windows x64   | `orbital-windows-x64.exe.gz` |

Linux builds are statically linked against musl, so they run on any distro
regardless of its glibc version.

</details>

## Usage

```sh
orbital                  # interactive menu: type to fuzzy-search, ↑/↓ to move, Enter to run, Esc to quit
orbital do show my ip    # say what you want in plain language; it picks the command
orbital ftp              # share the current directory over anonymous FTP (read-only, port 2121)
orbital ftp --write      # …and allow uploads, deletes and renames
orbital install          # copy this binary onto your PATH so `orbital` works anywhere
orbital ip               # list local interface addresses
orbital ip --local       # just the LAN IPv4, plain — e.g. IP=$(orbital ip --local)
orbital ip --public      # your public IP (looked up via an external service)
orbital qr "text"        # render text as a QR code; with no argument, build one interactively
echo "text" | orbital qr # …or pipe the payload in
orbital serve            # serve the current directory over HTTP (default port 8000)
orbital serve 8080       # …on a specific port; prints the LAN URL + a QR to scan
orbital sysinfo          # neofetch-style system info
ps aux | orbital table   # draw piped text as an aligned table
orbital update           # download & install the latest release
orbital --help
orbital --version
```

`orbital ip --local` (`-l`) and `--public` (`-p`) print a bare address with no UI
chrome, so they're safe to capture in scripts. Colour is dropped automatically when
output isn't a terminal, and when `NO_COLOR` is set, so piping and redirecting only
ever capture the command's own output.

### do

Describe what you want and `orbital` runs the matching command.

```sh
orbital do show a qr code for hello
orbital do what is my public ip
orbital do how much memory do i have
orbital do serve this folder on port 9000
echo "what os am i running" | orbital do

orbital do -n "share this folder"   # --dry-run: print the command, don't run it
orbital do -y "serve this folder"   # --yes: skip the confirmation for `serve`
```

It always prints the command it resolved to (`→ orbital qr hello`) before running
it, so nothing happens that you can't see. `serve` publishes the working directory
to your local network, so that one asks first unless you pass `--yes`.

**No network, no API key, no account.** The request is handled by a 45M-parameter
[Needle 2](https://github.com/cactus-compute/needle) model embedded in the binary,
running on one thread on your machine — roughly three seconds end to end, most of
it priming. Nothing is sent anywhere.

The call is *grammar-constrained while it is being decoded*, not checked
afterwards: a byte-level machine compiled from the tool schema restricts what the
sampler may emit, so the command is always one that exists and its arguments are
always the right type and shape. What the model can still get wrong is intent —
"tell me a joke" resolves to a QR code of "a joke" — which is why the resolved
command line is always shown.

Four commands are reachable: `qr`, `ip`, `sysinfo` and `serve`. That is a hard
ceiling, not a starting point — see [CLAUDE.md](./CLAUDE.md#the-four-tool-ceiling).
Everything else you run by name.

### ftp

Shares the current directory as an anonymous FTP server: any username gets in and
no password is asked. Handy for devices and apps that speak FTP but not HTTP —
file managers, TVs, media players, microcontrollers.

```sh
orbital ftp                 # read-only, port 2121
orbital ftp --write         # allow uploads, deletes, renames and new folders
orbital ftp 2121            # pick the port (bare number, --port 2121, or -p 2121)
```

Read-only is the default, so nothing on the network can change your files unless
you pass `--write` (`-w`). Every path is jailed to the shared directory, data
connections are only accepted from the machine that owns the session, and both
passive (PASV/EPSV) and active (PORT/EPRT) transfers are supported, so stock
clients — FileZilla, `curl`, file managers — work out of the box. Plain FTP is
unencrypted; use it on networks you trust.

### install

Already have the binary — built from source, or downloaded by hand — and want it on your PATH?

```sh
orbital install             # copy it to the default directory and put that on PATH
orbital install --dry-run   # say what would happen, change nothing
orbital install --dir ~/bin # install somewhere specific
orbital install --no-path   # copy it, but leave PATH alone
```

It installs **for the current user only**, so it never needs a password or an elevated prompt.

| Platform    | Default directory              | PATH                                              |
| ----------- | ------------------------------ | ------------------------------------------------- |
| Windows     | `%LOCALAPPDATA%\orbital\bin`   | appended to your user PATH, then open a new terminal |
| macOS/Linux | `/usr/local/bin` if writable, else `~/.local/bin` | prints the line to add to your shell profile |

`ORBITAL_INSTALL_DIR` works here exactly as it does for the install scripts. On Unix the command
prints a shell-appropriate `export`/`fish_add_path` line rather than editing your profile: guessing
the wrong rc file is a good way to break someone's login. On Windows it edits only
`HKCU\Environment`, preserving the value's existing type and any `%VAR%` references in it.

After installing it runs the installed copy once to confirm it actually executes, and warns if a
different `orbital` earlier on your PATH would still win.

### table

Reads delimited text from a pipe and prints it as a bordered, column-aligned table.
Columns whose cells are all numeric are right-aligned; short rows are padded out.

```sh
docker ps | orbital table --header      # first row becomes column headings
orbital table --csv < data.csv          # comma-separated, honouring "quoted, fields"
orbital table --tsv < data.tsv          # tab-separated
orbital table -d ';' < data.txt         # any single character ('\t' also accepted)
orbital table --ascii                   # +--+ borders instead of box-drawing
```

Without a delimiter flag, lines are split on runs of whitespace — the same rule
`awk` uses, which is what makes the output of most Unix tools line up.

### Updating

Run `orbital update` to check GitHub for a newer release and self-replace the
binary. Nothing checks for updates in the background — no banners, no cache, and no
network traffic unless you ask for it.

## Development

Requires a [Rust](https://rustup.rs) stable toolchain.

The first build downloads the 13.7 MB model weights into `model/` (git-ignored)
and checks them against their SHA-256; every build after that reuses the file.

```sh
cargo run -- <args>    # run the CLI from source
cargo test             # run the test suite
cargo clippy --all-targets -- -D warnings
cargo fmt
cargo build --release  # optimized binary in target/release/
```

## Adding a command

Every tool lives in its own module and is registered in one place. To add `mytool`:

1. Create `src/commands/mytool/`:
   - `mytool.rs`-style logic module (e.g. `addresses.rs`) — **pure functions** that
     take plain data rather than reading the world, so they're trivial to unit-test
     (see `ip/addresses.rs` and `sysinfo/info.rs`).
   - `mod.rs` — the `Command` descriptor plus a thin `view` that renders the logic's
     output. Tests for the rendering live alongside.
2. Add it to `COMMANDS` in `src/commands/mod.rs`.

It then automatically appears in `--help`, the interactive menu, and CLI dispatch.

See [CLAUDE.md](./CLAUDE.md) for the full conventions.
