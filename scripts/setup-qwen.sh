#!/usr/bin/env bash
set -euo pipefail
# An isolated runtime keeps Qwen's pinned Transformers out of system Python.
RUNTIME="${1:-${XDG_DATA_HOME:-$HOME/.local/share}/just-whisper/qwen-runtime}"
UV="${JUST_WHISPER_UV:-uv}"
if ! command -v "$UV" >/dev/null 2>&1; then
    if [[ -x "$HOME/.local/bin/uv" ]]; then UV="$HOME/.local/bin/uv";
    else printf 'Qwen kurulumu için uv gerekli: https://docs.astral.sh/uv/getting-started/installation/\n' >&2; exit 1; fi
fi
mkdir -p "$RUNTIME"
if [[ ! -x "$RUNTIME/bin/python" ]]; then
    "$UV" venv --python 3.12 "$RUNTIME"
fi
"$UV" pip install --python "$RUNTIME/bin/python" --torch-backend auto \
    'torch==2.10.0' 'qwen-asr==0.0.6'
"$RUNTIME/bin/python" -c 'import torch, numpy; from qwen_asr import Qwen3ASRModel'
printf 'qwen-asr=0.0.6 torch=2.10.0\n' > "$RUNTIME/.ready"
