use anyhow::{Context, Result, bail};
use std::process::{Command, Output};

const DEFAULT_SINK: &str = "@DEFAULT_AUDIO_SINK@";

#[derive(Clone, Copy)]
enum Backend {
    Wpctl,
    Pactl,
}

#[derive(Clone)]
pub struct SystemVolume {
    backend: Backend,
    volume: f64,
    muted: bool,
}

impl SystemVolume {
    /// Lower the default output volume for the duration of a recording.
    /// A value of 100 leaves the system untouched; zero mutes it.
    pub fn duck(percent: u8) -> Result<Option<Self>> {
        if percent >= 100 {
            return Ok(None);
        }

        let snapshot = Self::read(Backend::Wpctl).or_else(|_| Self::read(Backend::Pactl))?;
        let factor = f64::from(percent) / 100.0;
        let result = match snapshot.backend {
            Backend::Wpctl if percent == 0 => command("wpctl", &["set-mute", DEFAULT_SINK, "1"]),
            Backend::Wpctl => command(
                "wpctl",
                &[
                    "set-volume",
                    DEFAULT_SINK,
                    &format!("{:.4}", snapshot.volume * factor),
                ],
            ),
            Backend::Pactl if percent == 0 => {
                command("pactl", &["set-sink-mute", "@DEFAULT_SINK@", "1"])
            }
            Backend::Pactl => command(
                "pactl",
                &[
                    "set-sink-volume",
                    "@DEFAULT_SINK@",
                    &format!("{}%", (snapshot.volume * factor * 100.0).round() as u32),
                ],
            ),
        };
        if let Err(error) = result {
            let _ = snapshot.restore();
            return Err(error).context("Sistem sesi kısılamadı");
        }
        Ok(Some(snapshot))
    }

    fn read(backend: Backend) -> Result<Self> {
        let (volume, muted) = match backend {
            Backend::Wpctl => {
                let output = command("wpctl", &["get-volume", DEFAULT_SINK])?;
                let output = String::from_utf8_lossy(&output.stdout);
                let volume = output
                    .split_whitespace()
                    .find_map(|part| part.parse::<f64>().ok())
                    .context("wpctl ses düzeyi okunamadı")?;
                (volume, output.contains("[MUTED]"))
            }
            Backend::Pactl => {
                let output = command("pactl", &["get-sink-volume", "@DEFAULT_SINK@"])?;
                let output = String::from_utf8_lossy(&output.stdout);
                let percent = output
                    .split_whitespace()
                    .find_map(|part| part.strip_suffix('%')?.parse::<f64>().ok())
                    .context("pactl ses düzeyi okunamadı")?;
                let mute = command("pactl", &["get-sink-mute", "@DEFAULT_SINK@"])?;
                (
                    percent / 100.0,
                    String::from_utf8_lossy(&mute.stdout).contains("yes"),
                )
            }
        };
        Ok(Self {
            backend,
            volume,
            muted,
        })
    }

    fn restore(&self) -> Result<()> {
        match self.backend {
            Backend::Wpctl => {
                command(
                    "wpctl",
                    &["set-volume", DEFAULT_SINK, &format!("{:.4}", self.volume)],
                )?;
                command(
                    "wpctl",
                    &["set-mute", DEFAULT_SINK, if self.muted { "1" } else { "0" }],
                )?;
            }
            Backend::Pactl => {
                command(
                    "pactl",
                    &[
                        "set-sink-volume",
                        "@DEFAULT_SINK@",
                        &format!("{}%", (self.volume * 100.0).round() as u32),
                    ],
                )?;
                command(
                    "pactl",
                    &[
                        "set-sink-mute",
                        "@DEFAULT_SINK@",
                        if self.muted { "1" } else { "0" },
                    ],
                )?;
            }
        }
        Ok(())
    }
}

impl Drop for SystemVolume {
    fn drop(&mut self) {
        // A dropped guard must always try to restore the volume, including shutdown.
        if let Err(error) = self.restore() {
            eprintln!("Sistem ses düzeyi geri yüklenemedi: {error:#}");
        }
    }
}

fn command(program: &str, args: &[&str]) -> Result<Output> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("{program} çalıştırılamadı"))?;
    if !output.status.success() {
        bail!(
            "{program}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}
