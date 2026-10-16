use anyhow::{Context, Result, bail};
use gtk::glib;
use std::path::{Path, PathBuf};

fn entry_path() -> PathBuf {
    config_home().join(format!("autostart/{}.desktop", crate::APP_ID))
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
}

pub fn is_enabled() -> Result<bool> {
    read_enabled(&entry_path())
}

fn read_enabled(path: &Path) -> Result<bool> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("Başlangıç ayarı okunamadı"),
    };
    let entry = glib::KeyFile::new();
    entry
        .load_from_data(&contents, glib::KeyFileFlags::NONE)
        .context("Başlangıç ayarı okunamadı")?;
    Ok(!entry.boolean("Desktop Entry", "Hidden").unwrap_or(false)
        && entry
            .boolean("Desktop Entry", "X-GNOME-Autostart-enabled")
            .unwrap_or(true))
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    let executable = std::env::current_exe().context("Uygulamanın yolu bulunamadı")?;
    write_entry(&entry_path(), &executable, enabled)
        .context("Sistem açılışında başlatma ayarı uygulanamadı")
}

fn write_entry(path: &Path, executable: &Path, enabled: bool) -> Result<()> {
    if !enabled {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        };
    }
    let executable = executable.to_str().context("Uygulama yolu UTF-8 olmalı")?;
    if executable.contains(['\n', '\r']) {
        bail!("Uygulama yolu satır sonu içeremez");
    }
    // Exec quoting is separate from the key file's string escaping.
    let mut quoted = String::from("\"");
    for character in executable.chars() {
        match character {
            '\\' | '"' | '$' | '`' => {
                quoted.push('\\');
                quoted.push(character);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(character),
        }
    }
    quoted.push_str("\" --background");
    let entry = glib::KeyFile::new();
    entry.set_string("Desktop Entry", "Type", "Application");
    entry.set_string("Desktop Entry", "Name", "Just Whisper");
    entry.set_string("Desktop Entry", "Comment", "Konuşarak yaz");
    entry.set_string("Desktop Entry", "Exec", &quoted);
    entry.set_string("Desktop Entry", "Icon", "audio-input-microphone-symbolic");
    entry.set_boolean("Desktop Entry", "Terminal", false);
    entry.set_boolean("Desktop Entry", "StartupNotify", false);
    std::fs::create_dir_all(path.parent().context("Başlangıç dizini bulunamadı")?)?;
    let temp = path.with_extension("desktop.tmp");
    std::fs::write(&temp, entry.to_data())?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_lifecycle_and_exec_escaping() {
        let directory = std::env::temp_dir().join(format!(
            "just-whisper-autostart-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("autostart/app.desktop");
        assert!(!read_enabled(&path).unwrap());
        write_entry(&path, Path::new("/tmp/unused"), false).unwrap();
        let executable = Path::new("/tmp/Türkçe dizin/whisper\\\"$`%name");
        write_entry(&path, executable, true).unwrap();
        assert!(read_enabled(&path).unwrap());
        let entry = glib::KeyFile::new();
        entry
            .load_from_file(&path, glib::KeyFileFlags::NONE)
            .unwrap();
        assert_eq!(
            entry.string("Desktop Entry", "Exec").unwrap().as_str(),
            "\"/tmp/Türkçe dizin/whisper\\\\\\\"\\$\\`%%name\" --background"
        );
        entry.set_boolean("Desktop Entry", "Hidden", true);
        std::fs::write(&path, entry.to_data()).unwrap();
        assert!(!read_enabled(&path).unwrap());
        write_entry(&path, executable, true).unwrap();
        assert!(read_enabled(&path).unwrap());
        entry.set_boolean("Desktop Entry", "Hidden", false);
        entry.set_boolean("Desktop Entry", "X-GNOME-Autostart-enabled", false);
        std::fs::write(&path, entry.to_data()).unwrap();
        assert!(!read_enabled(&path).unwrap());
        write_entry(&path, executable, false).unwrap();
        assert!(!path.exists());
        assert!(!read_enabled(&path).unwrap());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
