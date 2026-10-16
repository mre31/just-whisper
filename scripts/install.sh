#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
if [[ "${1:-}" != "--no-build" ]]; then
    cargo build --release
fi
mkdir -p "$HOME/.local/bin"
BINARY="$(mktemp "$HOME/.local/bin/.just-whisper.XXXXXX")"
trap 'rm -f -- "$BINARY"' EXIT
install -m755 target/release/just-whisper "$BINARY"
mv -f -- "$BINARY" "$HOME/.local/bin/just-whisper"
install -Dm644 data/io.github.justwhisper.App.desktop "${XDG_DATA_HOME:-$HOME/.local/share}/applications/io.github.justwhisper.App.desktop"
install -Dm644 data/licenses/DeepFilterNet-MIT.txt "${XDG_DATA_HOME:-$HOME/.local/share}/just-whisper/licenses/DeepFilterNet-MIT.txt"
DESKTOP="${XDG_DATA_HOME:-$HOME/.local/share}/applications/io.github.justwhisper.App.desktop"
# The portal validates the desktop entry. Use an absolute executable path even
# when the desktop session's PATH does not contain ~/.local/bin.
EXEC_PATH="$HOME/.local/bin/just-whisper"
EXEC_PATH="${EXEC_PATH//\\/\\\\}"
EXEC_PATH="${EXEC_PATH//\"/\\\"}"
EXEC_PATH="${EXEC_PATH//\$/\\\$}"
EXEC_PATH="${EXEC_PATH//\`/\\\`}"
EXEC_PATH="${EXEC_PATH//%/%%}"
while IFS= read -r LINE; do
    if [[ "$LINE" == Exec=* ]]; then printf 'Exec="%s"\n' "$EXEC_PATH";
    else printf '%s\n' "$LINE"; fi
done < data/io.github.justwhisper.App.desktop > "$DESKTOP"
EXTENSION="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/just-whisper@local"
mkdir -p "$EXTENSION"
install -m644 gnome-extension/{extension.js,metadata.json} "$EXTENSION/"
if command -v update-desktop-database >/dev/null; then
    update-desktop-database "${XDG_DATA_HOME:-$HOME/.local/share}/applications"
fi
printf 'Uygulama kuruldu: %s\n' "$HOME/.local/bin/just-whisper"
if command -v gnome-extensions >/dev/null; then
    if ! gnome-extensions enable just-whisper@local; then
        CURRENT="$(gsettings get org.gnome.shell enabled-extensions)"
        CURRENT="${CURRENT#@as }"
        if [[ "$CURRENT" != *"'just-whisper@local'"* ]]; then
            if [[ "$CURRENT" == '[]' ]]; then CURRENT="['just-whisper@local']";
            else CURRENT="${CURRENT%]}, 'just-whisper@local']"; fi
            gsettings set org.gnome.shell enabled-extensions "$CURRENT"
        fi
        printf 'Overlay bir sonraki GNOME oturumu için etkinleştirildi. Oturumu kapatıp açın.\n'
    fi
fi
printf 'Uygulama açılışta ydotoold kullanıcı servisini otomatik başlatır.\n'
