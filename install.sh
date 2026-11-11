#!/usr/bin/env sh
# Builds Rite, installs it to ~/.local/bin and registers it as a text editor.
set -eu

cd "$(dirname "$0")"

BIN_DIR="$HOME/.local/bin"
APP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"

cargo build --release

mkdir -p "$BIN_DIR" "$APP_DIR"
install -m 755 target/release/rite "$BIN_DIR/rite"

cat > "$APP_DIR/rite.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Rite
Comment=A simple personal text editor
Exec=$BIN_DIR/rite %F
Terminal=false
Categories=Utility;TextEditor;
MimeType=text/plain;text/markdown;text/x-csrc;text/x-chdr;text/x-c++src;text/x-python;text/x-rust;text/x-shellscript;text/css;text/html;text/xml;application/json;application/x-shellscript;application/toml;application/yaml;
EOF

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APP_DIR"
fi

if command -v xdg-mime >/dev/null 2>&1; then
    for mime in text/plain text/markdown application/json application/x-shellscript; do
        xdg-mime default rite.desktop "$mime"
    done
fi

echo "Installed rite to $BIN_DIR/rite"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: $BIN_DIR is not in your PATH." ;;
esac
