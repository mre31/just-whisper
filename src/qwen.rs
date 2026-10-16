use crate::{config, process};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub const WORKER: &str = include_str!("../scripts/qwen-worker.py");
pub const SETUP: &str = include_str!("../scripts/setup-qwen.sh");

pub fn model_path() -> PathBuf {
    config::data_dir().join("models/qwen3-asr-1.7b")
}

pub fn runtime_dir() -> PathBuf {
    config::data_dir().join("qwen-runtime")
}

pub fn python() -> PathBuf {
    runtime_dir().join("bin/python")
}

#[derive(Debug, Deserialize)]
struct Response {
    kind: String,
    #[serde(default)]
    backend: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    error: String,
}

pub struct Qwen {
    child: Child,
    replies: mpsc::Receiver<Result<Response>>,
    reader: Option<thread::JoinHandle<()>>,
    pub backend: String,
}

impl Qwen {
    pub fn load(path: &Path, use_gpu: bool, cancel: &AtomicBool) -> Result<Self> {
        if !python().is_file() {
            bail!(
                "Qwen çalışma ortamı bulunamadı. Model listesinden Qwen3-ASR 1.7B'yi tekrar seçin."
            );
        }
        let mut command = Command::new(python());
        command
            .args(["-u", "-c", WORKER, "serve"])
            .arg(path)
            .env("JUST_WHISPER_PARENT_PID", std::process::id().to_string());
        if use_gpu {
            command.arg("--gpu");
        }
        Self::from_command(command, cancel)
    }

    fn from_command(mut command: Command, cancel: &AtomicBool) -> Result<Self> {
        // Linux PDEATHSIG follows the spawning thread, which exits after model
        // loading. The Python watchdog instead follows the application process.
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("Qwen süreci başlatılamadı")?;
        let stdout = child.stdout.take().unwrap();
        let (sender, replies) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let reply = line
                    .context("Qwen yanıtı okunamadı")
                    .and_then(|line| serde_json::from_str(&line).context("Geçersiz Qwen yanıtı"));
                if sender.send(reply).is_err() {
                    break;
                }
            }
        });
        let mut model = Self {
            child,
            replies,
            reader: Some(reader),
            backend: String::new(),
        };
        let reply = model.receive(cancel, Duration::from_secs(300))?;
        if reply.kind != "ready" {
            bail!("Qwen hazır yanıtı alınamadı");
        }
        model.backend = reply.backend;
        Ok(model)
    }

    fn receive(&mut self, cancel: &AtomicBool, timeout: Duration) -> Result<Response> {
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.load(Ordering::SeqCst) {
                self.stop();
                bail!("Qwen işlemi iptal edildi");
            }
            match self.replies.recv_timeout(Duration::from_millis(50)) {
                Ok(reply) => {
                    let reply = reply?;
                    if reply.kind == "error" {
                        self.stop();
                        bail!("Qwen: {}", reply.error);
                    }
                    return Ok(reply);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let status = self.child.wait().context("Qwen çıkış durumu okunamadı")?;
                    bail!("Qwen süreci durdu ({status}). Python hata çıktısını kontrol edin.");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if Instant::now() >= deadline {
                self.stop();
                bail!("Qwen işlemi zaman aşımına uğradı");
            }
        }
    }

    pub fn transcribe(
        &mut self,
        samples: &[f32],
        language: &str,
        abort: Arc<AtomicBool>,
    ) -> Result<String> {
        let mut input = self
            .child
            .stdin
            .take()
            .context("Qwen süreci yeniden yüklenmeli")?;
        let header = serde_json::to_vec(
            &serde_json::json!({"samples": samples.len(), "language": language}),
        )?;
        // Writing a full recording can block on the pipe. Keep cancellation responsive.
        let pcm: Vec<u8> = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let writer = thread::spawn(move || -> Result<_> {
            input.write_all(&header)?;
            input.write_all(b"\n")?;
            input.write_all(&pcm)?;
            input.flush()?;
            Ok(input)
        });
        let reply = self.receive(&abort, Duration::from_secs(600));
        if reply.is_err() {
            self.stop();
        }
        let written = writer
            .join()
            .map_err(|_| anyhow::anyhow!("Qwen ses aktarımı durdu"))?;
        let reply = reply?;
        self.child.stdin = Some(written.context("Ses Qwen'e aktarılamadı")?);
        if reply.kind != "result" {
            self.stop();
            bail!("Beklenmeyen Qwen yanıtı");
        }
        Ok(reply.text)
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for Qwen {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run setup/download commands in their own process group so cancellation also
/// stops installers and their children. Drain stderr continuously to avoid pipe stalls.
pub fn run_install(mut command: Command, cancel: &AtomicBool) -> Result<()> {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    process::stop_with_parent(&mut command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("Qwen kurulumu başlatılamadı")?;
    let stderr = child.stderr.take().unwrap();
    let reader = thread::spawn(move || {
        let mut tail = std::collections::VecDeque::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            eprintln!("{line}");
            if tail.len() == 12 {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        tail.into_iter().collect::<Vec<_>>().join("\n")
    });
    let started = Instant::now();
    let result = loop {
        if cancel.load(Ordering::SeqCst) || started.elapsed() > Duration::from_secs(3600) {
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = child.wait();
            break Err(anyhow::anyhow!(
                "Qwen indirmesi durduruldu. Tekrar seçerek devam edebilirsiniz."
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error.into());
            }
        }
    };
    let tail = reader.join().unwrap_or_default();
    if !result?.success() {
        bail!("Qwen kurulumu/indirmesi başarısız:\n{tail}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_reuses_model_and_preserves_unicode_results() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            r#"
            printf '%s\n' '{"kind":"ready","backend":"test CPU"}'
            while IFS= read -r header; do
                case "$header" in *'"samples":4'*) ;; *) exit 3 ;; esac
                dd bs=16 count=1 iflag=fullblock of=/dev/null 2>/dev/null
                printf '%s\n' '{"kind":"result","text":"Türkçe ığüşöç İĞÜŞÖÇ"}'
            done
        "#,
        ]);
        let abort = Arc::new(AtomicBool::new(false));
        let load_cancel = abort.clone();
        let mut model = thread::spawn(move || Qwen::from_command(command, &load_cancel))
            .join()
            .unwrap()
            .unwrap();
        assert_eq!(model.backend, "test CPU");
        for language in ["tr", "auto"] {
            assert_eq!(
                model
                    .transcribe(&[0.1, -0.2, 0.3, 0.0], language, abort.clone())
                    .unwrap(),
                "Türkçe ığüşöç İĞÜŞÖÇ"
            );
        }
    }

    #[test]
    fn cancellation_stops_worker_during_audio_transfer() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            r#"
            printf '%s\n' '{"kind":"ready","backend":"test CPU"}'
            IFS= read -r header
            IFS= read -r blocked
        "#,
        ]);
        let abort = Arc::new(AtomicBool::new(false));
        let mut model = Qwen::from_command(command, &abort).unwrap();
        let cancel = abort.clone();
        let timer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancel.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let result = model.transcribe(&vec![0.0; 16000 * 120], "tr", abort);
        timer.join().unwrap();
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(model.child.try_wait().unwrap().is_some());
    }

    #[test]
    fn model_load_errors_reach_the_application() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            r#"printf '%s\n' '{"kind":"error","error":"GPU belleği yetersiz"}'"#,
        ]);
        let error = Qwen::from_command(command, &AtomicBool::new(false))
            .err()
            .unwrap();
        assert!(error.to_string().contains("GPU belleği yetersiz"));
    }
}
