"""Local Qwen sidecar. stdin: JSON header + little-endian f32 PCM; stdout: JSON lines."""

import argparse
import contextlib
import json
import os
import sys
import threading
import time

MODEL_ID = "Qwen/Qwen3-ASR-1.7B"
MODEL_REVISION = "7278e1e70fe206f11671096ffdd38061171dd6e5"
MAX_SAMPLES = 16000 * 120


def emit(stream, **message):
    stream.write(json.dumps(message, ensure_ascii=False) + "\n")
    stream.flush()


def read_exact(stream, size):
    data = bytearray()
    while len(data) < size:
        chunk = stream.read(size - len(data))
        if not chunk:
            raise EOFError("Ses verisi tamamlanmadan bağlantı kapandı")
        data.extend(chunk)
    return data


def download(path):
    from pathlib import Path
    from huggingface_hub import snapshot_download

    snapshot_download(
        MODEL_ID, revision=MODEL_REVISION, local_dir=path, max_workers=2,
        ignore_patterns=[".gitattributes"],
    )
    Path(path, ".ready").write_text(MODEL_REVISION + "\n", encoding="utf-8")


def serve(path, use_gpu):
    # Keep library prints and progress bars away from the response protocol.
    protocol = sys.stdout
    with contextlib.redirect_stdout(sys.stderr):
        os.environ["HF_HUB_OFFLINE"] = "1"
        os.environ["TRANSFORMERS_OFFLINE"] = "1"
        os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
        import numpy as np
        import torch
        from qwen_asr import Qwen3ASRModel

        torch.set_num_threads(min(os.cpu_count() or 4, 8))
        cuda = use_gpu and torch.cuda.is_available()
        device = "cuda:0" if cuda else "cpu"
        dtype = (
            torch.bfloat16 if cuda and torch.cuda.is_bf16_supported()
            else torch.float16 if cuda else torch.float32
        )
        model = Qwen3ASRModel.from_pretrained(
            path, dtype=dtype, device_map=device,
            attn_implementation="sdpa", max_inference_batch_size=1,
            max_new_tokens=1024, local_files_only=True,
        )
        backend = f"GPU · {torch.cuda.get_device_name(0)} · CUDA" if cuda else "CPU · Qwen3-ASR"
        emit(protocol, kind="ready", backend=backend)
        source = sys.stdin.buffer
        while line := source.readline():
            try:
                request = json.loads(line)
                count = request["samples"]
                if not isinstance(count, int) or not 0 < count <= MAX_SAMPLES:
                    raise ValueError("Geçersiz ses uzunluğu")
                pcm = np.frombuffer(read_exact(source, count * 4), dtype="<f4").copy()
                language = request.get("language", "auto")
                if language == "auto":
                    language = None
                else:
                    language = {"tr": "Turkish", "en": "English"}[language]
                with torch.inference_mode():
                    results = model.transcribe(audio=(pcm, 16000), language=language)
                emit(protocol, kind="result", text=" ".join(results[0].text.split()))
            except Exception as error:
                emit(protocol, kind="error", error=str(error))
                # A framing error may leave unread PCM. Never reuse this process.
                raise


def main():
    parent = os.environ.get("JUST_WHISPER_PARENT_PID")
    if parent:
        parent = int(parent)

        def watch_parent():
            while os.getppid() == parent:
                time.sleep(0.5)
            os._exit(1)

        threading.Thread(target=watch_parent, daemon=True).start()
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["download", "serve"])
    parser.add_argument("path")
    parser.add_argument("--gpu", action="store_true")
    args = parser.parse_args()
    if args.mode == "download":
        download(args.path)
    else:
        try:
            serve(args.path, args.gpu)
        except Exception as error:
            emit(sys.stdout, kind="error", error=str(error))
            raise


if __name__ == "__main__":
    main()
