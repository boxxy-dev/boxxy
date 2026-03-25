#!/usr/bin/env sh
set -eu

# Boxxy Terminal Installation Script
# Installs to ~/.local/boxxy-terminal

REPO_OWNER="miifrommera"
REPO_NAME="boxxy"
INSTALL_DIR="$HOME/.local/boxxy-terminal"
BIN_DIR="$HOME/.local/bin"
DESKTOP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"

main() {
    platform="$(uname -s)"
    arch="$(uname -m)"

    if [ "$platform" != "Linux" ]; then
        echo "Boxxy Terminal currently only supports Linux."
        exit 1
    fi

    case "$arch" in
        x86_64)
            arch="x86_64"
            ;;
        aarch64 | arm64)
            arch="aarch64"
            ;;
        *)
            echo "Unsupported architecture: $arch"
            exit 1
            ;;
    esac

    # Create directories
    mkdir -p "$INSTALL_DIR" "$BIN_DIR" "$DESKTOP_DIR" "$ICON_DIR"

    # Temporary directory for download
    temp="$(mktemp -d "/tmp/boxxy-XXXXXX")"
    trap 'rm -rf "$temp"' EXIT

    echo "Downloading latest nightly for $arch..."
    
    # We target the 'nightly' tag
    ASSET_NAME="boxxy-terminal-nightly-linux-$arch.tar.gz"
    URL="https://github.com/$REPO_OWNER/$REPO_NAME/releases/download/nightly/$ASSET_NAME"

    if command -v curl >/dev/null 2>&1; then
        curl -fL "$URL" > "$temp/boxxy.tar.gz"
    elif command -v wget >/dev/null 2>&1; then
        wget -O "$temp/boxxy.tar.gz" "$URL"
    else
        echo "Could not find 'curl' or 'wget' in your path."
        exit 1
    fi

    echo "Extracting..."
    tar -xzf "$temp/boxxy.tar.gz" -C "$temp"

    # The archive contains a directory named boxxy-terminal-nightly-linux-$arch
    EXTRACTED_DIR=$(find "$temp" -maxdepth 1 -type d -name "boxxy-terminal-nightly*" | head -n 1)

    # Bypass "Text file busy" error if updating from within Boxxy itself
    if [ -f "$INSTALL_DIR/bin/boxxy-terminal" ]; then
        mv "$INSTALL_DIR/bin/boxxy-terminal" "$INSTALL_DIR/bin/boxxy-terminal.old" 2>/dev/null || true
    fi
    if [ -f "$INSTALL_DIR/bin/boxxy-agent" ]; then
        mv "$INSTALL_DIR/bin/boxxy-agent" "$INSTALL_DIR/bin/boxxy-agent.old" 2>/dev/null || true
    fi

    # Sync files to INSTALL_DIR
    cp -r "$EXTRACTED_DIR/"* "$INSTALL_DIR/"

    # Create symlinks
    echo "Creating symlinks..."
    ln -sf "$INSTALL_DIR/bin/boxxy-terminal" "$BIN_DIR/boxxy-terminal"
    ln -sf "$INSTALL_DIR/bin/boxxy-agent" "$BIN_DIR/boxxy-agent"

    # Setup Desktop File
    echo "Installing desktop entry..."
    DESKTOP_FILE="$DESKTOP_DIR/dev.boxxy.BoxxyTerminal.desktop"
    cp "$INSTALL_DIR/share/applications/dev.boxxy.BoxxyTerminal.desktop" "$DESKTOP_FILE"
    
    # Patch the desktop file with absolute paths
    sed -i "s|Exec=boxxy-terminal|Exec=$INSTALL_DIR/bin/boxxy-terminal|g" "$DESKTOP_FILE"
    sed -i "s|Icon=dev.boxxy.BoxxyTerminal|Icon=$INSTALL_DIR/share/icons/hicolor/scalable/apps/dev.boxxy.BoxxyTerminal.png|g" "$DESKTOP_FILE"

    # Update icon cache (if tool exists)
    if command -v gtk4-update-icon-cache >/dev/null 2>&1; then
        gtk4-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
    fi

    echo ""
    echo "Boxxy Terminal has been installed successfully!"
    echo "You can run it with: boxxy-terminal"

    # Check if BIN_DIR is in PATH
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *)
            echo ""
            echo "NOTE: $BIN_DIR is not in your PATH."
            echo "Please add it to your shell config (~/.bashrc, ~/.zshrc, or config.fish):"
            echo "export PATH=\"\$HOME/.local/bin:\$PATH\""
            ;;
    esac
}

main "$@"
