# CLAUDE.md

Conventions for working in this repo. Keep it clean and consistent.

## What this is

`orbital` is a single cross-platform CLI binary that aggregates many small tools.
It is written in Rust and ships as one self-contained executable per platform.
That executable also carries a 45M-parameter language model, which is what makes
`orbital do "show my ip"` work with no network and no API key.

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
- **Natural language:** `src/core/needle/` is a from-scratch Rust port of the
  Needle 2 inference engine; `src/commands/do/` is the thin command over it. See
  [The `do` command](#the-do-command) — it has invariants that fail *silently*.
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

## The `do` command

`orbital do <plain language>` runs a 45M-parameter Needle 2 model locally and
dispatches the tool call it produces to a real command. `src/core/needle/` is a
port of the Go engine in `needle-2-test/needle/`, itself a port of the portable
C99 reference. **When a question about intended behaviour comes up, read the C
reference, not this port.**

### Validating a change to the engine

Unit tests cover the grammar machine, the kernels and the dispatch rules. They do
**not** cover whether the forward pass is still correct — a numerics bug produces
plausible wrong tokens, not a failure. The check that catches that is running the
same queries through both engines and diffing the decisions:

```sh
# in needle-2-test/
./build/ndtest model/needle2.cact <schema.json> "what is my ip" 96 nothink
# in orbital/
orbital do --dry-run "what is my ip"
```

The port currently agrees with the reference on 16/16 queries, down to
reproducing the same *invented* port numbers — which only happens if every argmax
along the chain matches. Treat any divergence as a real bug.

### The four-tool ceiling

`TOOLS_JSON` in `src/commands/do/tools.rs` declares exactly four tools, and that
is a measured limit rather than a preference. The schema is rendered at the head
of the prompt and shares a **256-token sliding window** with the request; four
tools already cost 182 of it. Measured against the reference engine:

| Tools | Prefix tokens | Behaviour |
|---|---|---|
| 3 | 123 | 7/7 correct |
| 4 | 182 | 9/11 correct — the shipped schema |
| 5 | 207 | fires `sysinfo` at "what is the capital of france" |

Adding a fifth tool degrades the four that are there. Upstream solves this with a
tool-retrieval head that neither this port nor the C reference implements.

**Tool descriptions are load-bearing**, not documentation: the wording is what
routes "how much memory do i have" to `sysinfo`. Re-measure after editing one.

### Invariants that fail silently

Beyond the ones already in the engine's module docs:

- **Tensor binding is positional.** `.cact` carries no tensor names, so the bind
  order in `model.rs` *is* the format. An off-by-one produces garbage, not an
  error.
- **The schema must be compacted** before the model sees it. Needle was trained
  on `json.dumps(separators=(",",":"))` output; an indented schema is far enough
  off-distribution that the model starts citing tools that were never declared,
  with no error anywhere. `Session::new` calls `json_compact`; do not bypass it.
- **The int8 KV cache rounds half to even** (`round_ties_even`), matching C's
  `lrintf`. Rounding half away from zero flipped one generation in ten in the Go
  port before it was caught, and unit tests did not see it.
- **`GState` must stay `Copy` with no owned fields.** The sampler copies it once
  per candidate token to trial-run bytes; a `Vec` or a pointer in there turns a
  cheap copy into aliasing and corrupts the grammar.
- **`fast_exp` is not interchangeable with `f64::exp`.** It replicates the
  reference's `nd_expf` and is used exactly where C uses `nd_expf`; `pool_cell`
  uses `f64::exp` because C uses libm `expf` there. Keep the split.
- **The engine's arithmetic must wrap.** The engram hash relies on `uint32`
  overflow, so it uses `wrapping_mul` — Rust would otherwise panic in debug.
- **Unsupported schema constructs fail at compile time.** Nested objects and
  arrays are rejected in `compile` rather than passed through unenforced, so we
  never believe we are enforcing a schema we are not. Do not make it permissive.

### The model blob

`build.rs` fetches `model/needle2.cact` (13.7 MB, git-ignored), verifies its
SHA-256, and `src/commands/do/mod.rs` embeds it with `include_bytes!`. There is
no runtime download, no cache directory and no model file to find — which is why
`do` still works offline. CI caches `model/` keyed on that same SHA. Changing the
blob means updating `MODEL_SHA256`, `MODEL_BYTES` and the workflow cache keys
together.

The model may only ever act *after* the resolved command line has been printed,
and `serve` — the one command that publishes the working directory to the local
network — is confirmed before it runs. Generation is grammar-constrained, so a
call is always well-formed; what it can still get wrong is intent, and the user
has to be able to see that.

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
