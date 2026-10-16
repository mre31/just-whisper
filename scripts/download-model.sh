#!/usr/bin/env bash
set -euo pipefail
MODEL="${1:-base}"
[[ "$MODEL" != large ]] || MODEL=large-v3
case "$MODEL" in tiny|base|small|medium|large-v3|large-v3-turbo|tiny.en|base.en|small.en|medium.en) ;; *) printf 'Geçersiz Whisper modeli: %s\n' "$MODEL" >&2; exit 1;; esac
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/just-whisper/models"
mkdir -p "$DEST"
FILE="$DEST/ggml-$MODEL.bin"
if [[ -f "$FILE" ]]; then printf 'Model mevcut: %s\n' "$FILE"; exit 0; fi
curl --fail --location --retry 2 --continue-at - --connect-timeout 20 --max-time 1800 \
    --output "$FILE.part" "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$MODEL.bin"
mv -- "$FILE.part" "$FILE"
printf 'Model indirildi: %s\n' "$FILE"
