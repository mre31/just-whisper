use anyhow::{Context, Result, bail};
use rustfft::{FftPlanner, num_complex::Complex};
use std::{
    io::Read,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

pub const SAMPLE_RATE: usize = 16_000;
pub const CAPTURE_RATE: usize = 48_000;
pub const MAX_SECONDS: usize = 120;
pub const BANDS: usize = 21;
const LEVEL_WINDOW: usize = 1536;

#[derive(Default)]
pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub levels: Vec<f64>,
    pub finished: bool,
    pub error: Option<String>,
}

pub struct Capture {
    child: Child,
    reader: Option<JoinHandle<()>>,
    stopped: Arc<AtomicBool>,
    pub buffer: Arc<Mutex<AudioBuffer>>,
}

/// PCM framing is independent of pipe read boundaries.
#[derive(Default)]
struct Decoder {
    pending: Vec<u8>,
}
impl Decoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<f32> {
        self.pending.extend_from_slice(bytes);
        let count = self.pending.len() / 4 * 4;
        let decoded = self.pending[..count]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .map(|v| {
                if v.is_finite() {
                    v.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect();
        self.pending.drain(..count);
        decoded
    }
}

impl Capture {
    pub fn start() -> Result<Self> {
        let mut command = Command::new("ffmpeg");
        command.args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "pulse",
            "-fragment_size",
            "1024",
            "-i",
            "default",
            "-ac",
            "1",
            "-ar",
            "48000",
            "-f",
            "f32le",
            "-flush_packets",
            "1",
            "pipe:1",
        ]);
        Self::from_command(command)
    }

    pub(crate) fn from_command(mut command: Command) -> Result<Self> {
        crate::process::stop_with_parent(&mut command);
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("Mikrofon kaydı başlatılamadı. ffmpeg kurulu mu?")?;
        let mut pipe = child.stdout.take().unwrap();
        let buffer = Arc::new(Mutex::new(AudioBuffer::default()));
        let output = buffer.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let reader = thread::spawn(move || {
            let mut bytes = [0u8; 2048];
            let mut decoder = Decoder::default();
            let mut planner = FftPlanner::new();
            let fft = planner.plan_fft_forward(LEVEL_WINDOW);
            loop {
                let n = match pipe.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => {
                        output.lock().unwrap().error = Some(e.to_string());
                        break;
                    }
                };
                let samples = decoder.push(&bytes[..n]);
                let mut data = output.lock().unwrap();
                let remaining = CAPTURE_RATE * MAX_SECONDS - data.samples.len();
                data.samples.extend(samples.into_iter().take(remaining));
                if data.samples.len() >= LEVEL_WINDOW {
                    let mut spectrum: Vec<_> = data.samples[data.samples.len() - LEVEL_WINDOW..]
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            Complex::new(
                                *v * (0.5
                                    - 0.5
                                        * (std::f32::consts::TAU * i as f32
                                            / (LEVEL_WINDOW - 1) as f32)
                                            .cos()),
                                0.0,
                            )
                        })
                        .collect();
                    fft.process(&mut spectrum);
                    data.levels = (0..BANDS)
                        .map(|i| {
                            let start = 2 + i * 5;
                            let energy = spectrum[start..start + 5]
                                .iter()
                                .map(|v| v.norm())
                                .sum::<f32>()
                                / 5.0;
                            (energy as f64 / 24.0).sqrt().clamp(0.0, 1.0)
                        })
                        .collect();
                }
                if data.samples.len() == CAPTURE_RATE * MAX_SECONDS {
                    break;
                }
            }
            let mut data = output.lock().unwrap();
            data.finished = true;
            if !stop.load(Ordering::SeqCst)
                && data.samples.len() < CAPTURE_RATE * MAX_SECONDS
                && data.error.is_none()
            {
                data.error = Some(
                    "Mikrofon kaydı beklenmedik biçimde durdu. ffmpeg çıktısını kontrol edin."
                        .into(),
                );
            }
        });
        Ok(Self {
            child,
            reader: Some(reader),
            stopped,
            buffer,
        })
    }

    pub fn finish(mut self) -> Result<Vec<f32>> {
        self.stop();
        let mut data = self.buffer.lock().unwrap();
        if let Some(error) = &data.error {
            bail!("{error}");
        }
        Ok(std::mem::take(&mut data.samples))
    }

    fn stop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        // SIGINT lets ffmpeg flush the last PCM packet before its pipe closes.
        // Do not signal a PID after it has been reaped and potentially reused.
        if matches!(self.child.try_wait(), Ok(None)) {
            unsafe {
                libc::kill(self.child.id() as libc::pid_t, libc::SIGINT);
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while matches!(self.child.try_wait(), Ok(None)) && std::time::Instant::now() < deadline
            {
                thread::sleep(std::time::Duration::from_millis(10));
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn has_speech(samples: &[f32], sample_rate: usize) -> bool {
    // A short tap or digital silence must not trigger Whisper hallucinations.
    samples.len() >= sample_rate / 4
        && samples
            .chunks(sample_rate / 50)
            .any(|chunk| chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32 > 0.000_004)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_audio_before_model_ready_across_partial_reads() {
        let expected = [0.1f32, -0.3, 0.8, -0.9, 0.0];
        let bytes: Vec<_> = expected.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut decoder = Decoder::default();
        let decoded: Vec<_> = bytes
            .chunks(3)
            .flat_map(|chunk| decoder.push(chunk))
            .collect();
        assert_eq!(decoded, expected);
    }
    #[test]
    fn silence_and_taps_are_rejected_but_quiet_speech_survives() {
        for rate in [SAMPLE_RATE, CAPTURE_RATE] {
            assert!(!has_speech(&vec![0.0; rate], rate));
            assert!(!has_speech(&vec![0.8; rate / 100], rate));
            assert!(has_speech(&vec![0.01; rate], rate));
        }
    }
}
