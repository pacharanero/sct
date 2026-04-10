#!/bin/sh
# sct installer — downloads the latest prebuilt binary from GitHub Releases,
# verifies its SHA-256 checksum, and installs it to ~/.local/bin by default.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/pacharanero/sct/main/install.sh | sh
#
# Environment variables:
#   SCT_INSTALL_DIR   Override install directory (default: $HOME/.local/bin)
#   SCT_VERSION       Install a specific version tag, e.g. v0.3.7 (default: latest)

set -eu

REPO="pacharanero/sct"
INSTALL_DIR="${SCT_INSTALL_DIR:-$HOME/.local/bin}"

err() { printf 'error: %s\n' "$*" >&2; exit 1; }
info() { printf '%s\n' "$*"; }

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Linux)
            case "$arch" in
                x86_64|amd64)   echo "linux-x86_64" ;;
                aarch64|arm64)  echo "linux-aarch64" ;;
                *) err "unsupported Linux architecture: $arch" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                x86_64)         echo "macos-x86_64" ;;
                arm64)          echo "macos-aarch64" ;;
                *) err "unsupported macOS architecture: $arch" ;;
            esac
            ;;
        *) err "unsupported OS: $os — try 'cargo install sct-rs' instead" ;;
    esac
}

fetch() {
    url="$1"
    out="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$out"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$out" "$url"
    else
        err "neither curl nor wget is installed"
    fi
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        err "neither sha256sum nor shasum is installed"
    fi
}

latest_version() {
    tmp="$1"
    fetch "https://api.github.com/repos/$REPO/releases/latest" "$tmp"
    # Parse tag_name without requiring jq
    grep '"tag_name":' "$tmp" \
        | head -1 \
        | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
}

main() {
    target="$(detect_target)"

    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT

    version="${SCT_VERSION:-}"
    if [ -z "$version" ]; then
        info "Looking up latest sct release..."
        version="$(latest_version "$tmpdir/release.json")"
        [ -n "$version" ] || err "could not determine latest version"
    fi
    info "Installing sct $version for $target"

    archive="sct-$target.tar.gz"
    url="https://github.com/$REPO/releases/download/$version/$archive"
    checksums_url="https://github.com/$REPO/releases/download/$version/SHA256SUMS"

    info "Downloading $archive..."
    fetch "$url" "$tmpdir/$archive"

    info "Verifying SHA-256 checksum..."
    fetch "$checksums_url" "$tmpdir/SHA256SUMS"
    expected="$(grep " $archive\$" "$tmpdir/SHA256SUMS" | awk '{print $1}')"
    [ -n "$expected" ] || err "checksum for $archive not found in SHA256SUMS"
    actual="$(sha256_of "$tmpdir/$archive")"
    if [ "$expected" != "$actual" ]; then
        err "checksum mismatch:
  expected: $expected
  got:      $actual"
    fi
    info "Checksum OK"

    info "Extracting..."
    tar -xzf "$tmpdir/$archive" -C "$tmpdir"

    mkdir -p "$INSTALL_DIR"
    mv "$tmpdir/sct" "$INSTALL_DIR/sct"
    chmod +x "$INSTALL_DIR/sct"

    info ""
    info "sct installed to $INSTALL_DIR/sct"
    info ""

    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            info "$INSTALL_DIR is not on your PATH. Add it with:"
            info ""
            info "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.bashrc"
            info "  # or ~/.zshrc, ~/.profile, ~/.config/fish/config.fish — whichever your shell uses"
            info ""
            ;;
    esac

    "$INSTALL_DIR/sct" --version || true
}

main "$@"
