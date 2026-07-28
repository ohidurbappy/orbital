# orb

A growable, cross-platform CLI toolbox — an aggregate of small tools that share one
binary, one update mechanism, and one consistent UI (built with [Ink](https://github.com/vadimdemedes/ink)).

## Install

**macOS / Linux** — paste into your terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/ohidurbappy/orb/main/install.sh | sh
```

**Windows** — paste into PowerShell:

```powershell
irm https://raw.githubusercontent.com/ohidurbappy/orb/main/install.ps1 | iex
```

The installer detects your platform, downloads the latest release, and installs
it as `orb` on your `PATH`. Override the location with `ORB_INSTALL_DIR` if you
like. Re-run it any time to upgrade (or use `orb update`).

<details>
<summary>Manual install</summary>

Grab the `.gz` asset for your platform from the
[latest release](https://github.com/ohidurbappy/orb/releases/latest), decompress
it, mark it executable, and put it on your `PATH`:

```sh
# example: macOS arm64
curl -fsSL https://github.com/ohidurbappy/orb/releases/latest/download/orb-darwin-arm64.gz | gunzip > orb
chmod +x orb
sudo mv orb /usr/local/bin/orb
```

| Platform      | Asset                      |
| ------------- | -------------------------- |
| macOS (Apple) | `orb-darwin-arm64.gz`      |
| macOS (Intel) | `orb-darwin-x64.gz`        |
| Linux x64     | `orb-linux-x64.gz`         |
| Linux arm64   | `orb-linux-arm64.gz`       |
| Windows x64   | `orb-windows-x64.exe.gz`   |

</details>

## Usage

```sh
orb                  # interactive menu: type to fuzzy-search, ↑/↓ to move, Enter to run, Esc to quit
orb ip               # list local interface addresses
orb ip --local       # just the LAN IPv4, plain — e.g. IP=$(orb ip --local)
orb ip --public      # your public IP (looked up via an external service)
orb serve            # serve the current directory over HTTP (default port 8000)
orb serve 8080       # …on a specific port; prints the LAN URL + a QR to scan
orb sysinfo          # neofetch-style system info
orb update           # download & install the latest release
orb --help
orb --version
```

`orb ip --local` (`-l`) and `--public` (`-p`) print a bare address with no UI
chrome, so they're safe to capture in scripts.

orb checks GitHub for a newer release on startup (and every 10 minutes while the menu
is open). When one is found it shows a banner; run `orb update` to self-replace the
binary. Checks are cached at `~/.config/orb/state.json` and never block a command.

## Development

Requires [Bun](https://bun.sh) ≥ 1.3.

```sh
bun install
bun run dev          # run the CLI from source
bun test             # run the test suite
bun run typecheck    # tsc --noEmit
bun run build        # cross-compile all targets into dist/
bun run build darwin-arm64   # build a single target
```

## Adding a command

Every tool lives in its own folder and is registered in one place. To add `mytool`:

1. Create `src/commands/mytool/`:
   - `mytool.ts` — **pure logic**, with side-effecting dependencies passed as
     parameters (see `ip.ts` / `sysinfo.ts`) so it's trivial to unit-test.
   - `MytoolCommand.tsx` — a thin Ink component that renders the logic's output.
   - `index.ts` — export a `Command` descriptor.
   - `mytool.test.ts` / `MytoolCommand.test.tsx` — tests.
2. Add it to the registry in `src/commands/index.ts`.

It then automatically appears in `--help`, the interactive menu, and CLI dispatch.

See [CLAUDE.md](./CLAUDE.md) for the full conventions.
