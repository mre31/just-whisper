use anyhow::{Context, Result, bail};
use std::{
    io::Write,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

pub fn start_paste_service() -> Result<()> {
    let result = Command::new("systemctl")
        .args(["--user", "start", "ydotoold.service"])
        .stdin(Stdio::null())
        .output()
        .context("Yapıştırma servisi başlatılamadı. systemctl gerekli.")?;
    if !result.status.success() {
        bail!(
            "Yapıştırma servisi başlatılamadı. ydotoold kullanıcı servisini ve /dev/uinput erişimini kontrol edin. {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(())
}

/// Clipboard + a physical paste chord preserves Turkish/Unicode on any layout.
pub fn insert(text: &str, paste_shortcut: &str) -> Result<()> {
    let text = text.trim_end();
    if text.is_empty() {
        return Ok(());
    }
    let text = format!("{text} ");
    let mut copy = Command::new("wl-copy")
        .args(["--type", "text/plain;charset=utf-8"])
        .stdin(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("wl-copy başlatılamadı")?;
    let write = copy.stdin.take().unwrap().write_all(text.as_bytes());
    // wl-copy forks a clipboard owner. Wait for the original process only;
    // a captured stderr pipe can otherwise stay open in the clipboard owner.
    let result = copy.wait()?;
    write.context("Metin panoya gönderilemedi")?;
    if !result.success() {
        bail!("Panoya kopyalama başarısız. wl-copy çıktısını kontrol edin.");
    }
    // wl-copy may briefly focus a helper surface. Its parent exits before
    // the compositor restores focus and delivers the new clipboard offer.
    thread::sleep(Duration::from_millis(150));
    let keys = if paste_shortcut == "ctrl-shift-v" {
        vec!["29:1", "42:1", "47:1", "47:0", "42:0", "29:0"]
    } else {
        vec!["29:1", "47:1", "47:0", "29:0"]
    };
    let result = Command::new("ydotool")
        .args(["key", "--key-delay", "25"])
        .args(keys)
        .output()
        .context("ydotool başlatılamadı. Metin panoda.")?;
    if !result.status.success() {
        bail!(
            "ydotool yazamadı. Metin panoda; elle yapıştırabilirsiniz. ydotoold servisini kontrol edin. {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;
    use std::time::{Duration, Instant};

    struct ClipboardCleanup;

    impl Drop for ClipboardCleanup {
        fn drop(&mut self) {
            let _ = Command::new("wl-copy")
                .arg("--clear")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    fn pump_until(mut ready: impl FnMut() -> bool) -> bool {
        let context = gtk::glib::MainContext::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            while context.pending() {
                context.iteration(false);
            }
            if ready() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    #[ignore = "Requires a Wayland desktop and ydotoold; opens a test window and changes the clipboard"]
    fn repeatedly_pastes_unicode_into_focused_entry() {
        gtk::init().unwrap();
        let entry = gtk::Entry::new();
        let window = gtk::Window::builder()
            .title("Just Whisper paste verification")
            .default_width(500)
            .child(&entry)
            .build();
        window.present();
        entry.grab_focus();
        assert!(pump_until(|| window.is_active()), "Test window needs focus");
        let _clipboard_cleanup = ClipboardCleanup;
        for i in 0..20 {
            entry.set_text("");
            let text = format!("{i}: Türkçe ığüşöç İĞÜŞÖÇ 🎤");
            let expected = format!("{text} ");
            let worker = std::thread::spawn(move || insert(&text, "ctrl-v"));
            assert!(
                pump_until(|| worker.is_finished()),
                "Paste command timed out"
            );
            worker.join().unwrap().unwrap();
            assert!(
                pump_until(|| entry.text() == expected),
                "Paste {i} failed: expected {expected:?}, got {:?}, window active: {}",
                entry.text(),
                window.is_active()
            );
        }
        window.close();
    }
}
