#!/bin/sh
# voyager (vygr) installer.
#
# Prefers a prebuilt release binary from GitHub releases; falls back to
# building from source with cargo. Usage:
#   curl -fsSL <raw-url>/install.sh | sh
#   ./install.sh [--version v0.4.0] [--prefix ~/.local]

set -eu

REPO="${VYGR_REPO:-jaltez/voyager}"
VERSION="${VYGR_VERSION:-latest}"
PREFIX="${VYGR_PREFIX:-$HOME/.local}"

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --prefix) PREFIX="$2"; shift 2 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS-$ARCH" in
    Linux-x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
    Linux-aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
    Darwin-x86_64) TARGET="x86_64-apple-darwin" ;;
    Darwin-arm64) TARGET="aarch64-apple-darwin" ;;
    *) TARGET="" ;;
esac

install_dir="$PREFIX/bin"
mkdir -p "$install_dir"

if [ -n "$TARGET" ] && command -v curl >/dev/null 2>&1; then
    if [ "$VERSION" = "latest" ]; then
        url="https://github.com/$REPO/releases/latest/download/vygr-$TARGET.tar.gz"
    else
        url="https://github.com/$REPO/releases/download/$VERSION/vygr-$TARGET.tar.gz"
    fi
    tmp="$(mktemp -d)"
    echo "trying prebuilt binary: $url"
    if curl -fsSL "$url" -o "$tmp/vygr.tar.gz" 2>/dev/null; then
        tar -xzf "$tmp/vygr.tar.gz" -C "$tmp"
        mv "$tmp/vygr" "$install_dir/vygr"
        chmod +x "$install_dir/vygr"
        rm -rf "$tmp"
        echo "installed: $install_dir/vygr"
        "$install_dir/vygr" --version
        exit 0
    fi
    rm -rf "$tmp"
    echo "no prebuilt binary found; building from source"
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found — install Rust from https://rustup.rs first" >&2
    exit 1
fi
echo "building vygr from source (cargo install)..."
cargo install --locked --path "$(dirname "$0")/crates/cli" --root "$PREFIX" --bin vygr
"$install_dir/vygr" --version
echo "installed: $install_dir/vygr (make sure $install_dir is on your PATH)"
