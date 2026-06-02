#!/usr/bin/env bash
#
# FanController installer (Linux).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Smokey-thc/FanController/main/install.sh | bash
#
# Override the repo if you forked it:
#   FANCONTROLLER_REPO=you/FanController bash install.sh
#
set -euo pipefail

REPO="${FANCONTROLLER_REPO:-Smokey-thc/FanController}"
BRANCH="${FANCONTROLLER_BRANCH:-main}"
PREFIX="${PREFIX:-/usr/local/bin}"

say()  { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[!]\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m[x]\033[0m %s\n' "$*" >&2; exit 1; }

# ── 1. Install build dependencies via the detected package manager ──────────────
install_deps() {
    if command -v pacman >/dev/null 2>&1; then
        say "Arch detected — installing dependencies (pacman)..."
        sudo pacman -S --needed --noconfirm rust gtk3 webkit2gtk-4.1 nvidia-utils \
            || warn "Some packages may be missing (e.g. nvidia-utils without an NVIDIA GPU — that's fine)."
    elif command -v apt >/dev/null 2>&1; then
        say "Debian/Ubuntu detected — installing dependencies (apt)..."
        sudo apt update
        sudo apt install -y rustc cargo libgtk-3-dev libwebkit2gtk-4.1-dev \
            || sudo apt install -y rustc cargo libgtk-3-dev libwebkit2gtk-4.0-dev
    elif command -v dnf >/dev/null 2>&1; then
        say "Fedora detected — installing dependencies (dnf)..."
        sudo dnf install -y rust cargo gtk3-devel webkit2gtk4.1-devel \
            || sudo dnf install -y rust cargo gtk3-devel webkit2gtk3-devel
    else
        warn "Unknown package manager. Please install manually: rust/cargo, gtk3, webkit2gtk-4.1."
    fi
}

# ── 2. Make sure cargo is on PATH (rustup installs to ~/.cargo) ─────────────────
ensure_cargo() {
    if command -v cargo >/dev/null 2>&1; then return; fi
    if [ -x "$HOME/.cargo/bin/cargo" ]; then
        export PATH="$HOME/.cargo/bin:$PATH"; return
    fi
    say "Rust not found — installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    export PATH="$HOME/.cargo/bin:$PATH"
    command -v cargo >/dev/null 2>&1 || die "cargo still not found after rustup install."
}

# ── 3. Fetch source ─────────────────────────────────────────────────────────────
fetch_source() {
    WORKDIR="$(mktemp -d)"
    trap 'rm -rf "$WORKDIR"' EXIT
    say "Downloading source from $REPO ($BRANCH)..."
    if command -v git >/dev/null 2>&1; then
        git clone --depth 1 --branch "$BRANCH" "https://github.com/$REPO.git" "$WORKDIR/FanController"
    else
        curl -fsSL "https://github.com/$REPO/archive/refs/heads/$BRANCH.tar.gz" \
            | tar -xz -C "$WORKDIR"
        mv "$WORKDIR"/FanController-* "$WORKDIR/FanController"
    fi
    SRC="$WORKDIR/FanController"
}

# ── 4. Build & install ──────────────────────────────────────────────────────────
build_install() {
    say "Building release binary (this may take a few minutes)..."
    ( cd "$SRC" && cargo build --release )
    say "Installing to $PREFIX/fancontroller (requires sudo)..."
    sudo install -Dm755 "$SRC/target/release/fancontroller" "$PREFIX/fancontroller"
}

main() {
    [ "$(uname -s)" = "Linux" ] || die "This installer is Linux only. Windows support is coming."
    install_deps
    ensure_cargo
    fetch_source
    build_install
    echo
    say "Done! Start FanController with:  \033[1mfancontroller\033[0m"
    warn "On first launch FanController will ask for your sudo password once (permission setup)."
}

main "$@"
