use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HotkeyMode {
    #[default]
    Portal,
    RightAlt,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AsrBackend {
    #[default]
    Whisper,
    Qwen3,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub asr_backend: AsrBackend,
    pub model_path: PathBuf,
    pub language: String,
    pub load_on_press: bool,
    pub model_keep_alive_secs: u64,
    pub overlay: bool,
    pub paste_shortcut: String,
    pub use_gpu: bool,
    pub noise_reduction: bool,
    pub system_volume_percent: u8,
    pub hotkey_mode: HotkeyMode,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            asr_backend: AsrBackend::Whisper,
            model_path: data_dir().join("models/ggml-base.bin"),
            language: "tr".into(),
            load_on_press: true,
            model_keep_alive_secs: 300,
            overlay: true,
            paste_shortcut: "ctrl-v".into(),
            use_gpu: true,
            noise_reduction: true,
            system_volume_percent: 100,
            hotkey_mode: HotkeyMode::Portal,
        }
    }
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
        .join("just-whisper")
}

pub fn config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
        .join("just-whisper/config.json")
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        serde_json::from_slice(&std::fs::read(&path)?).context("Ayar dosyası okunamadı")
    }

    pub fn validate(&self) -> Result<()> {
        if self.model_keep_alive_secs > 86_400 {
            bail!("Modeli bellekte tutma süresi en fazla 24 saat olabilir");
        }
        let model_exists = match self.asr_backend {
            AsrBackend::Whisper => self.model_path.is_file(),
            AsrBackend::Qwen3 => {
                self.model_path.join(".ready").is_file()
                    && self.model_path.join("config.json").is_file()
                    && self
                        .model_path
                        .join("model.safetensors.index.json")
                        .is_file()
            }
        };
        if !model_exists {
            bail!("Model bulunamadı: {}", self.model_path.display());
        }
        if self.language != "auto" && whisper_rs::get_lang_id(&self.language).is_none() {
            bail!("Geçersiz dil kodu: {}", self.language);
        }
        if self.asr_backend == AsrBackend::Qwen3
            && !["tr", "en", "auto"].contains(&self.language.as_str())
        {
            bail!("Qwen için Türkçe, İngilizce veya Otomatik dil seçin");
        }
        if !["ctrl-v", "ctrl-shift-v"].contains(&self.paste_shortcut.as_str()) {
            bail!("Geçersiz yapıştırma kısayolu");
        }
        if self.system_volume_percent > 100 {
            bail!("Sistem sesi yüzdesi 0 ile 100 arasında olmalı");
        }
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        std::fs::create_dir_all(path.parent().unwrap())?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(temp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn noise_reduction_defaults_on_and_can_be_disabled() {
        let legacy: Config = serde_json::from_str(r#"{"language":"tr"}"#).unwrap();
        assert_eq!(legacy.asr_backend, AsrBackend::Whisper);
        assert!(legacy.noise_reduction);
        let disabled: Config = serde_json::from_str(r#"{"noise_reduction":false}"#).unwrap();
        let saved = serde_json::to_vec(&disabled).unwrap();
        assert!(
            !serde_json::from_slice::<Config>(&saved)
                .unwrap()
                .noise_reduction
        );
    }

    #[test]
    fn qwen_backend_and_model_directory_survive_serialization() {
        let qwen = Config {
            asr_backend: AsrBackend::Qwen3,
            model_path: PathBuf::from("/models/qwen3-asr-1.7b"),
            ..Config::default()
        };
        let loaded: Config = serde_json::from_slice(&serde_json::to_vec(&qwen).unwrap()).unwrap();
        assert_eq!(loaded.asr_backend, AsrBackend::Qwen3);
        assert_eq!(loaded.model_path, qwen.model_path);
    }

    #[test]
    fn legacy_configs_keep_models_for_five_minutes_and_zero_disables_caching() {
        let legacy: Config = serde_json::from_str(r#"{"load_on_press":true}"#).unwrap();
        assert_eq!(legacy.model_keep_alive_secs, 300);
        let disabled: Config = serde_json::from_str(r#"{"model_keep_alive_secs":0}"#).unwrap();
        let loaded: Config =
            serde_json::from_slice(&serde_json::to_vec(&disabled).unwrap()).unwrap();
        assert_eq!(loaded.model_keep_alive_secs, 0);
    }
    #[test]
    fn old_settings_enable_gpu_and_explicit_cpu_choice_survives() {
        let legacy: Config = serde_json::from_str(r#"{"language":"tr","overlay":false}"#).unwrap();
        assert!(legacy.use_gpu);
        assert!(!legacy.overlay);
        assert_eq!(legacy.hotkey_mode, HotkeyMode::Portal);
        let cpu: Config = serde_json::from_str(r#"{"use_gpu":false}"#).unwrap();
        assert!(!cpu.use_gpu);
        let reloaded: Config = serde_json::from_slice(&serde_json::to_vec(&cpu).unwrap()).unwrap();
        assert!(!reloaded.use_gpu);
        let altgr: Config = serde_json::from_str(r#"{"hotkey_mode":"right-alt"}"#).unwrap();
        assert_eq!(altgr.hotkey_mode, HotkeyMode::RightAlt);
        assert_eq!(
            serde_json::from_slice::<Config>(&serde_json::to_vec(&altgr).unwrap())
                .unwrap()
                .hotkey_mode,
            HotkeyMode::RightAlt
        );
    }
}
