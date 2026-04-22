#!/usr/bin/env bash
set -euo pipefail

BINARY_NAME="taskpilot"
LOCAL_BIN="./target/release/$BINARY_NAME"
GLOBAL_DIR="${TASKPILOT_INSTALL_DIR:-$HOME/.local/bin}"

usage() {
    cat <<EOF
Usage: ./install.sh [OPTIONS]

Install taskpilot locally (project) or globally (user PATH).

Options:
  --local       Build and keep binary in ./target/release/ (default)
  --global      Build and copy binary to $GLOBAL_DIR
  --dir <path>  Override global install directory
  --debug       Build in debug mode instead of release
  -h, --help    Show this help

Environment:
  TASKPILOT_INSTALL_DIR   Override default global install path (~/.local/bin)

Examples:
  ./install.sh                      # local release build
  ./install.sh --global             # install to ~/.local/bin
  ./install.sh --global --dir /usr/local/bin
EOF
    exit 0
}

MODE="local"
PROFILE="release"
CUSTOM_DIR=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --local)  MODE="local"; shift ;;
        --global) MODE="global"; shift ;;
        --dir)    CUSTOM_DIR="$2"; shift 2 ;;
        --debug)  PROFILE="debug"; shift ;;
        -h|--help) usage ;;
        *) echo "Unknown option: $1"; usage ;;
    esac
done

if [[ -n "$CUSTOM_DIR" ]]; then
    GLOBAL_DIR="$CUSTOM_DIR"
fi

# Check for cargo
if ! command -v cargo &>/dev/null; then
    echo "Error: cargo not found. Install Rust from https://rustup.rs" >&2
    exit 1
fi

# Build
echo "Building $BINARY_NAME ($PROFILE)..."
if [[ "$PROFILE" == "release" ]]; then
    cargo build --release
    BUILT="./target/release/$BINARY_NAME"
else
    cargo build
    BUILT="./target/debug/$BINARY_NAME"
fi

echo "Built: $BUILT"

if [[ "$MODE" == "local" ]]; then
    echo ""
    echo "Local install complete. Run with:"
    echo "  $BUILT"
    exit 0
fi

# Global install
mkdir -p "$GLOBAL_DIR"
cp "$BUILT" "$GLOBAL_DIR/$BINARY_NAME"
chmod +x "$GLOBAL_DIR/$BINARY_NAME"

echo ""
echo "Installed to $GLOBAL_DIR/$BINARY_NAME"

# Check if on PATH
if ! echo "$PATH" | tr ':' '\n' | grep -qx "$GLOBAL_DIR"; then
    echo ""
    echo "Warning: $GLOBAL_DIR is not on your PATH."
    echo "Add it with:"
    echo ""
    SHELL_NAME="$(basename "$SHELL")"
    case "$SHELL_NAME" in
        zsh)  echo "  echo 'export PATH=\"$GLOBAL_DIR:\$PATH\"' >> ~/.zshrc && source ~/.zshrc" ;;
        bash) echo "  echo 'export PATH=\"$GLOBAL_DIR:\$PATH\"' >> ~/.bashrc && source ~/.bashrc" ;;
        fish) echo "  fish_add_path $GLOBAL_DIR" ;;
        *)    echo "  export PATH=\"$GLOBAL_DIR:\$PATH\"" ;;
    esac
fi

echo ""
echo "Verify: $BINARY_NAME --help"
