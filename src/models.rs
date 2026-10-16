use crate::{
    config::{self, AsrBackend},
    qwen,
};
use anyhow::{Context, Result, bail};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub struct ModelSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub bytes: u64,
    pub backend: AsrBackend,
}

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "tiny",
        title: "Whisper Tiny · 75 MB",
        bytes: 78_000_000,
        backend: AsrBackend::Whisper,
    },
    ModelSpec {
        id: "base",
        title: "Whisper Base · 142 MB",
        bytes: 148_000_000,
        backend: AsrBackend::Whisper,
    },
    ModelSpec {
        id: "small",
        title: "Whisper Small · 466 MB",
        bytes: 488_000_000,
        backend: AsrBackend::Whisper,
    },
    ModelSpec {
        id: "medium",
        title: "Whisper Medium · 1,5 GB",
        bytes: 1_534_000_000,
        backend: AsrBackend::Whisper,
    },
    ModelSpec {
        id: "large-v3",
        title: "Whisper Large v3 · 3,1 GB",
        bytes: 3_095_000_000,
        backend: AsrBackend::Whisper,
    },
    ModelSpec {
        id: "large-v3-turbo",
        title: "Whisper Large v3 Turbo · 1,6 GB",
        bytes: 1_625_000_000,
        backend: AsrBackend::Whisper,
    },
    ModelSpec {
        id: "qwen3-asr-1.7b",
        title: "Qwen3-ASR 1.7B",
        bytes: 0,
        backend: AsrBackend::Qwen3,
    },
];

impl ModelSpec {
    pub fn path(&self) -> PathBuf {
        match self.backend {
            AsrBackend::Whisper => config::data_dir().join(format!("models/ggml-{}.bin", self.id)),
            AsrBackend::Qwen3 => qwen::model_path(),
        }
    }
}

pub struct Progress {
    pub text: String,
    pub fraction: Option<f64>,
}

pub fn prepare(
    spec: &ModelSpec,
    cancel: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<PathBuf> {
    let path = spec.path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    if cancel.load(Ordering::SeqCst) {
        bail!("İndirme durduruldu");
    }
    if spec.backend == AsrBackend::Qwen3 {
        if !qwen::runtime_dir().join(".ready").is_file() || !qwen::python().is_file() {
            progress(Progress {
                text: "Qwen çalışma ortamı kuruluyor… İlk kurulum birkaç GB indirebilir.".into(),
                fraction: None,
            });
            let mut setup = Command::new("bash");
            setup
                .args(["-c", qwen::SETUP, "setup-qwen"])
                .arg(qwen::runtime_dir());
            qwen::run_install(setup, cancel)?;
        }
        if !path.join(".ready").is_file() {
            progress(Progress {
                text: "Qwen3-ASR 1.7B indiriliyor…".into(),
                fraction: None,
            });
            let mut download = Command::new(qwen::python());
            download
                .args(["-u", "-c", qwen::WORKER, "download"])
                .arg(&path)
                .env("HF_HUB_DISABLE_TELEMETRY", "1");
            qwen::run_install(download, cancel)?;
        }
        return Ok(path);
    }
    if path.is_file() {
        return Ok(path);
    }
    let partial = path.with_extension("bin.part");
    let mut command = Command::new("curl");
    command
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--continue-at",
            "-",
            "--retry",
            "2",
            "--connect-timeout",
            "20",
            "--max-time",
            "1800",
            "--output",
        ])
        .arg(&partial)
        .arg(format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
            spec.id
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    crate::process::stop_with_parent(&mut command);
    let mut child = command
        .spawn()
        .context("Model indirmek için curl gerekli")?;
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("İndirme durduruldu. Aynı modeli tekrar seçerek devam edebilirsiniz.");
        }
        let bytes = std::fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
        progress(Progress {
            text: format!("{} MB indirildi", bytes / 1_000_000),
            fraction: Some((bytes as f64 / spec.bytes as f64).min(1.0)),
        });
        if child.try_wait()?.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let result = child.wait_with_output()?;
    if !result.status.success() {
        bail!(
            "Model indirilemedi: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    std::fs::rename(&partial, &path)?;
    Ok(path)
}
