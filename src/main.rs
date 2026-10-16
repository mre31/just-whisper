mod audio;
mod autostart;
mod config;
mod engine;
mod evdev_hotkey;
mod hotkey;
mod models;
mod noise;
mod output;
mod process;
mod qwen;
mod residency;
mod system_volume;
mod ui;

use gtk::{gio, prelude::*};

pub const APP_ID: &str = "io.github.justwhisper.App";
pub const OBJECT_PATH: &str = "/io/github/justwhisper/App";
pub const OVERLAY_IFACE: &str = "io.github.justwhisper.Overlay";

fn main() -> gtk::glib::ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--transcribe") {
        let result = (|| -> anyhow::Result<String> {
            let audio = args
                .get(index + 1)
                .ok_or_else(|| anyhow::anyhow!("--transcribe için ses dosyası gerekli"))?;
            let mut config = config::Config::load()?;
            if let Some(index) = args.iter().position(|arg| arg == "--backend") {
                config.asr_backend = match args.get(index + 1).and_then(|arg| arg.to_str()) {
                    Some("whisper") => config::AsrBackend::Whisper,
                    Some("qwen3") => config::AsrBackend::Qwen3,
                    _ => anyhow::bail!("--backend whisper veya qwen3 olmalı"),
                };
                config.model_path = match config.asr_backend {
                    config::AsrBackend::Whisper => config::Config::default().model_path,
                    config::AsrBackend::Qwen3 => qwen::model_path(),
                };
            }
            if args.iter().any(|arg| arg == "--no-noise-reduction") {
                config.noise_reduction = false;
            }
            if let Some(index) = args.iter().position(|arg| arg == "--language") {
                config.language = args
                    .get(index + 1)
                    .ok_or_else(|| anyhow::anyhow!("Dil kodu gerekli"))?
                    .to_string_lossy()
                    .into();
            }
            if let Some(index) = args.iter().position(|arg| arg == "--model") {
                config.model_path = args
                    .get(index + 1)
                    .ok_or_else(|| anyhow::anyhow!("Model yolu gerekli"))?
                    .into();
            }
            engine::transcribe_file(&config, std::path::Path::new(audio))
        })();
        return match result {
            Ok(text) => {
                println!("{text}");
                gtk::glib::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e:#}");
                gtk::glib::ExitCode::FAILURE
            }
        };
    }
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_command_line(|app, command| {
        let args = command.arguments();
        let background = args.iter().any(|a| a == "--background");
        if app.windows().is_empty() {
            ui::build(app, background);
        } else if !background {
            app.activate();
        }
        gtk::glib::ExitCode::SUCCESS
    });
    app.connect_activate(|app| {
        if let Some(window) = app.windows().first() {
            window.present();
        }
    });
    app.run()
}
