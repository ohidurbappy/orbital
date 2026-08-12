#!/bin/sh
# orbital installer for macOS and Linux.
#
# Downloads the latest release binary for your platform and installs it as
# `orbital` on your PATH. Usage:
#
#   curl -fsSL https://raw.githubusercontent.com/ohidurbappy/orbital/main/install.sh | sh
#
# Override the install directory with ORBITAL_INSTALL_DIR=/path sh install.sh
set -eu

REPO="ohidurbappy/orbital"
BIN="orbital"

err() {
  printf 'orbital-install: error: %s\n' "$*" >&2
  exit 1
}

# --- detect platform ---------------------------------------------------------
os="$(uname -s)"
case "$os" in
  Darwin) os="darwin" ;;
  Linux) os="linux" ;;
  *) err "unsupported OS '$os' — use install.ps1 on Windows" ;;
esac

arch="$(uname -m)"
case "$arch" in
  x86_64 | amd64) arch="x64" ;;
  arm64 | aarch64) arch="arm64" ;;
  *) err "unsupported architecture '$arch'" ;;
esac

asset="${BIN}-${os}-${arch}.gz"
url="https://github.com/${REPO}/releases/latest/download/${asset}"

# --- pick a downloader -------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  download() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  download() { wget -qO "$2" "$1"; }
else
  err "need curl or wget installed"
fi

# --- choose install directory ------------------------------------------------
if [ -n "${ORBITAL_INSTALL_DIR:-}" ]; then
  dir="$ORBITAL_INSTALL_DIR"
elif [ -w /usr/local/bin ]; then
  dir="/usr/local/bin"
else
  dir="$HOME/.local/bin"
fi
mkdir -p "$dir" || err "cannot create install directory: $dir"

# --- download, decompress, install -------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

printf 'Downloading %s ...\n' "$asset"
download "$url" "$tmp/$BIN.gz" || err "download failed: $url"
gunzip -c "$tmp/$BIN.gz" > "$tmp/$BIN" || err "failed to decompress $asset"
chmod +x "$tmp/$BIN"

target="$dir/$BIN"
if mv "$tmp/$BIN" "$target" 2>/dev/null; then
  :
elif command -v sudo >/dev/null 2>&1; then
  printf 'Writing %s requires elevated permissions...\n' "$target"
  sudo mv "$tmp/$BIN" "$target" || err "failed to install to $target"
else
  err "cannot write to $target — set ORBITAL_INSTALL_DIR to a writable directory"
fi

printf '\nInstalled orbital to %s\n' "$target"

# --- PATH hint ---------------------------------------------------------------
case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    printf '\nNote: %s is not on your PATH. Add it to your shell profile:\n' "$dir"
    printf '  export PATH="%s:$PATH"\n' "$dir"
    ;;
esac

printf '\nRun: orbital --help\n'
