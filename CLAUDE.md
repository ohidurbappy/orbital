# CLAUDE.md

Conventions for working in this repo. Keep it clean and consistent.

## What this is

`orbital` is a single cross-platform CLI binary that aggregates many small tools.
It is written in Rust and ships as one self-contained executable per platform.

## Architecture

- **Command registry** (`src/commands/mod.rs`) is the single source of truth.
  `COMMANDS` drives `--help`, the interactive menu, and CLI dispatch. Register new
  commands there.
- **Folder per command** under `src/commands/<name>/`:
  - a logic module (`addresses.rs`, `encode.rs`, `files.rs`, `info.rs`) — **pure
    functions over plain data**. Side effects are read once at the edge and passed
    in as values (see `sysinfo::info::Raw` and `ip::addresses::collect_ips`), so
    tests pass fixtures instead of mocking the OS.
  - `mod.rs` — the `Command` descriptor, a `view` that renders, and an optional
    `run` for plain-output invocations. No business logic.
  - tests live in `#[cfg(test)] mod tests` alongside the code they cover.
- **Rendering is separated from logic.** A view builds a `Vec<String>` of lines and
  hands it to `term::emit` (print once) or `term::Frame` (repaint in place). Render
  functions are pure so they can be asserted on directly. Shared pieces live in
  `src/components/` (`key_value`, `menu`).
- **Entry/dispatch:** `src/main.rs` parses argv, resolves the command, reads piped
  stdin for commands that opt in, tries the plain-output `run` path, then calls
  `view`. With no command it opens the menu.
- **Terminal:** `src/term.rs` owns raw mode (`RawMode` restores it on drop, and
  nests safely), key classification, and frame redrawing. `src/style.rs` owns ANSI
  colour, which turns itself off for non-terminals and `NO_COLOR`.
- **Writing binaries:** `src/core/binfile.rs` owns `place_executable`, shared by `update` (replaces
  the running binary) and `install` (writes a copy elsewhere). Never `fs::copy` an executable: it
  carries alternate data streams and the read-only attribute on Windows, and xattrs including
  `com.apple.quarantine` on macOS. Always stage a fresh file *in the destination directory* and
  rename, so the swap is atomic and can't cross a filesystem.
- **Update:** `src/core/updater/` runs only when the user asks. `orbital update`
  calls `apply`, which checks GitHub via `check` and self-replaces the binary.
  There is deliberately no background check, no cached state, and no banner —
  nothing touches the network unless the user ran `update`.

## Platform-specific code

- `src/commands/install/winenv.rs` holds the **only `unsafe` in the crate** — the
  `HKCU\Environment` registry read/write and the `WM_SETTINGCHANGE` broadcast. Keep it free of
  decisions: planning belongs in `install/plan.rs`, which is pure and testable on every OS.
- Never write the Windows PATH with `setx` (truncates at 1024 characters) or with .NET's
  `SetEnvironmentVariable` (expands `%VAR%` on read and demotes the value to `REG_SZ`, freezing the
  machine PATH into the user's). Read the raw UTF-16, append, write it back with its `kind` intact.
- Prefer a runtime `cfg!(windows)` branch over `#[cfg(windows)]` so both sides keep compiling
  everywhere. CI runs `clippy -D warnings` on Linux, macOS *and* Windows, so an item reachable on
  only one platform is a build failure on the others — either keep it reachable or annotate it
  (`#[cfg_attr(not(windows), allow(dead_code))]`) and say why.
- Build paths with the separator of the OS being *planned for*, not the host's: `Path::join` uses the
  host separator, so `install/plan.rs` has its own `join` and the tests assert Windows layouts while
  running on Linux.

## Conventions

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must
  all pass. CI enforces all three.
- Errors in update/network paths are swallowed into safe results — they must never
  break the command the user actually ran.
- Interactive views must work when stdin/stdout aren't a terminal: check
  `Ctx::interactive` and fall back to printing a single frame (see `qr` and the
  menu), never block on a key read that can't come.
- Only a command's own output reaches stdout, so `ps | orbital table > out.txt`
  captures the table and nothing else. Don't add banners or notices to it.
- `term::Frame` must stay flicker-free: an unchanged frame writes nothing, and a
  repaint overwrites rows in place (erase-to-end-of-line) instead of blanking the
  block first. Interactive loops block on a key rather than waking on a timer to
  repaint something that hasn't changed.
- Commands declare how they want piped input with `Stdin`: `WhenNoArgs` when the
  arguments are the payload (`qr`), `Always` when they're options (`table`).
- Asset names in `src/core/updater/assets.rs` must match the names produced by
  `.github/workflows/release.yml`. Release assets are **gzipped** (`<name>.gz`);
  the workflow gzips each binary and `apply_update` gunzips on download.

## Commands

```sh
cargo run -- <args>    # run from source
cargo test             # tests
cargo clippy --all-targets -- -D warnings
cargo fmt
cargo build --release  # optimized binary
```

## Release

**Every push to `main` cuts a release** (`.github/workflows/release.yml`): it gates
on fmt + clippy + tests, then builds each target on a native runner (Linux x64/arm64
as static musl, macOS arm64/x64, Windows x64), gzips them, generates
`checksums.txt`, and creates the GitHub release.

Versioning is automatic. `Cargo.toml`'s `version` supplies `MAJOR.MINOR`; CI appends
the workflow run number as the patch and passes the result as `ORBITAL_VERSION`,
which `src/core/version.rs` prefers over `CARGO_PKG_VERSION`. Every push therefore
ships a strictly-increasing semver with no manual bump. To start a new major/minor
series, edit `version` in `Cargo.toml`.
