use crate::{
    config::HotkeyMode,
    engine::{self, Controller, Event},
};
use anyhow::{Context, Result};
use evdev::{Device, EventType, KeyCode};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    os::fd::AsRawFd,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::{Duration, Instant},
};

const RIGHT_ALT: u16 = KeyCode::KEY_RIGHTALT.code();

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Start,
    Stop,
    Cancel,
}

#[derive(Default)]
struct AltGr {
    held: HashMap<usize, HashSet<u16>>,
    recording: bool,
    blocked: bool,
}

impl AltGr {
    fn right_alt_held(&self) -> bool {
        self.held.values().any(|keys| keys.contains(&RIGHT_ALT))
    }

    fn key(&mut self, device: usize, key: u16, value: i32, locked: bool) -> Option<Action> {
        if value != 0 && value != 1 {
            return None;
        }
        let keys = self.held.entry(device).or_default();
        let changed = if value == 1 {
            keys.insert(key)
        } else {
            keys.remove(&key)
        };
        if !changed {
            return None;
        }
        if !self.right_alt_held() {
            self.blocked = false;
            return self.recording.then(|| {
                self.recording = false;
                Action::Stop
            });
        }
        // AltGr combinations such as AltGr+Q keep typing their normal symbol.
        if locked
            || self
                .held
                .values()
                .any(|keys| keys.iter().any(|key| *key != RIGHT_ALT))
        {
            self.blocked = true;
            return self.cancel();
        }
        if key == RIGHT_ALT && value == 1 && !self.recording && !self.blocked {
            self.recording = true;
            Some(Action::Start)
        } else {
            None
        }
    }

    fn cancel(&mut self) -> Option<Action> {
        self.recording.then(|| {
            self.recording = false;
            self.blocked = self.right_alt_held();
            Action::Cancel
        })
    }

    fn remove(&mut self, device: usize) -> Option<Action> {
        let held_alt = self
            .held
            .remove(&device)
            .is_some_and(|keys| keys.contains(&RIGHT_ALT));
        let action = if held_alt { self.cancel() } else { None };
        if !self.right_alt_held() {
            self.blocked = false;
        }
        action
    }
}

struct Keyboard {
    id: usize,
    path: PathBuf,
    device: Device,
}

fn open_keyboard(path: &std::path::Path) -> std::io::Result<Option<Device>> {
    // Read-only access. Never grab the keyboard or change its event delivery.
    let file = File::open(path)?;
    let device = Device::from_fd(file.into())?;
    if !device
        .supported_keys()
        .is_some_and(|keys| keys.contains(KeyCode::KEY_RIGHTALT) && keys.contains(KeyCode::KEY_A))
        || device
            .name()
            .is_some_and(|name| name.to_ascii_lowercase().contains("ydotool"))
    {
        return Ok(None);
    }
    device.set_nonblocking(true)?;
    Ok(Some(device))
}

pub fn run(
    controller: Controller,
    events: Sender<Event>,
    modes: tokio::sync::watch::Receiver<Option<HotkeyMode>>,
    locked: Arc<AtomicBool>,
) -> Result<()> {
    let mut keyboards: Vec<Keyboard> = Vec::new();
    let mut next_id = 0;
    let mut scan = Instant::now() - Duration::from_secs(1);
    let mut reported_missing = false;
    let mut altgr = AltGr::default();
    let _ = events.send(Event::Shortcut("Sağ AltGr · evdev".into()));
    while *modes.borrow() == Some(HotkeyMode::RightAlt) && modes.has_changed().is_ok() {
        if locked.load(Ordering::SeqCst) && altgr.cancel().is_some() {
            controller.cancel();
        }
        if scan.elapsed() >= Duration::from_secs(1) {
            scan = Instant::now();
            let had_keyboard = !keyboards.is_empty();
            if let Ok(entries) = std::fs::read_dir("/dev/input") {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !entry.file_name().to_string_lossy().starts_with("event")
                        || keyboards.iter().any(|keyboard| keyboard.path == path)
                    {
                        continue;
                    }
                    if let Ok(Some(device)) = open_keyboard(&path) {
                        keyboards.push(Keyboard {
                            id: next_id,
                            path,
                            device,
                        });
                        next_id += 1;
                    }
                }
            }
            if keyboards.is_empty() && !reported_missing {
                let _ = events.send(Event::Error(
                    "AltGr için klavye okunamadı. /dev/input erişimini kontrol edin. Gerekirse sudo usermod -aG input \"$USER\" çalıştırıp oturumu kapatıp açın veya masaüstü kısayolunu seçin.".into(),
                ));
                reported_missing = true;
            } else if !keyboards.is_empty() && (reported_missing || !had_keyboard) {
                let _ = events.send(Event::Status(
                    "Sağ AltGr hazır. Basılı tut, konuş, bırak.".into(),
                ));
                reported_missing = false;
            }
        }
        let mut poll: Vec<_> = keyboards
            .iter()
            .map(|keyboard| libc::pollfd {
                fd: keyboard.device.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        // Poll wakes immediately on key events; the timeout bounds shutdown and lock checks.
        let ready = unsafe { libc::poll(poll.as_mut_ptr(), poll.len() as libc::nfds_t, 100) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if altgr.cancel().is_some() {
                controller.cancel();
            }
            return Err(error).context("Klavye olayları okunamadı");
        }
        if *modes.borrow() != Some(HotkeyMode::RightAlt) || modes.has_changed().is_err() {
            break;
        }
        let mut removed = Vec::new();
        for (index, descriptor) in poll.iter().enumerate() {
            if descriptor.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                removed.push(index);
                continue;
            }
            if descriptor.revents & libc::POLLIN == 0 {
                continue;
            }
            let keyboard = &mut keyboards[index];
            match keyboard.device.fetch_events() {
                Ok(batch) => {
                    for event in batch {
                        if event.event_type() != EventType::KEY {
                            continue;
                        }
                        let action = altgr.key(
                            keyboard.id,
                            event.code(),
                            event.value(),
                            locked.load(Ordering::SeqCst),
                        );
                        match action {
                            Some(Action::Start) => {
                                if controller.phase.load(Ordering::SeqCst) == engine::IDLE {
                                    controller.start();
                                } else {
                                    altgr.recording = false;
                                    altgr.blocked = true;
                                }
                            }
                            Some(Action::Stop) => controller.stop(),
                            Some(Action::Cancel) => controller.cancel(),
                            None => {}
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => removed.push(index),
            }
        }
        for index in removed.into_iter().rev() {
            let keyboard = keyboards.remove(index);
            if altgr.remove(keyboard.id).is_some() {
                controller.cancel();
            }
        }
    }
    if altgr.cancel().is_some() {
        controller.cancel();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Requires /dev/input access and a running ydotoold; emits an AltGr press/release"]
    fn kernel_events_preserve_right_alt_press_and_release() {
        let mut physical = 0;
        let mut virtual_keyboard = None;
        for entry in std::fs::read_dir("/dev/input").unwrap().flatten() {
            let path = entry.path();
            if !entry.file_name().to_string_lossy().starts_with("event") {
                continue;
            }
            if open_keyboard(&path).is_ok_and(|device| device.is_some()) {
                physical += 1;
            }
            let Ok(file) = File::open(&path) else {
                continue;
            };
            let Ok(device) = Device::from_fd(file.into()) else {
                continue;
            };
            if device
                .name()
                .is_some_and(|name| name.to_ascii_lowercase().contains("ydotool"))
            {
                assert!(
                    open_keyboard(&path).unwrap().is_none(),
                    "Ignore our own paste device"
                );
                device.set_nonblocking(true).unwrap();
                virtual_keyboard = Some(device);
            }
        }
        assert!(physical > 0, "No readable physical keyboard");
        let mut device = virtual_keyboard.expect("ydotoold virtual keyboard missing");
        assert!(
            std::process::Command::new("ydotool")
                .args(["key", "100:1", "100:0"])
                .status()
                .unwrap()
                .success()
        );
        let mut state = AltGr::default();
        let mut actions = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while actions.len() < 2 && Instant::now() < deadline {
            match device.fetch_events() {
                Ok(batch) => {
                    for event in batch {
                        if event.event_type() == EventType::KEY
                            && let Some(action) = state.key(0, event.code(), event.value(), false)
                        {
                            actions.push(action);
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        }
        assert_eq!(actions, vec![Action::Start, Action::Stop]);
    }

    #[test]
    fn right_alt_hold_ignores_left_alt_and_repeat() {
        let mut state = AltGr::default();
        assert_eq!(state.key(0, KeyCode::KEY_LEFTALT.code(), 1, false), None);
        assert_eq!(state.key(0, KeyCode::KEY_LEFTALT.code(), 0, false), None);
        assert_eq!(state.key(0, RIGHT_ALT, 1, false), Some(Action::Start));
        assert_eq!(state.key(0, RIGHT_ALT, 2, false), None);
        assert_eq!(state.key(0, RIGHT_ALT, 1, false), None);
        assert_eq!(state.key(0, RIGHT_ALT, 0, false), Some(Action::Stop));
        assert_eq!(state.key(0, RIGHT_ALT, 0, false), None);
    }

    #[test]
    fn altgr_symbols_cancel_dictation_without_restarting_while_held() {
        let mut state = AltGr::default();
        assert_eq!(state.key(0, RIGHT_ALT, 1, false), Some(Action::Start));
        assert_eq!(
            state.key(0, KeyCode::KEY_Q.code(), 1, false),
            Some(Action::Cancel)
        );
        assert_eq!(state.key(0, KeyCode::KEY_Q.code(), 0, false), None);
        assert_eq!(state.key(0, RIGHT_ALT, 0, false), None);
        assert_eq!(state.key(0, RIGHT_ALT, 1, false), Some(Action::Start));
        assert_eq!(state.key(0, RIGHT_ALT, 0, false), Some(Action::Stop));
    }

    #[test]
    fn multiple_keyboards_lock_and_disconnect_do_not_leave_recording_active() {
        let mut state = AltGr::default();
        assert_eq!(state.key(0, RIGHT_ALT, 1, false), Some(Action::Start));
        assert_eq!(state.key(1, RIGHT_ALT, 1, false), None);
        assert_eq!(state.key(0, RIGHT_ALT, 0, false), None);
        assert_eq!(state.remove(1), Some(Action::Cancel));
        assert_eq!(state.key(0, RIGHT_ALT, 1, true), None);
        assert_eq!(state.key(0, RIGHT_ALT, 0, false), None);
        assert_eq!(state.key(0, RIGHT_ALT, 1, false), Some(Action::Start));
        assert_eq!(state.cancel(), Some(Action::Cancel));
        assert_eq!(state.key(0, RIGHT_ALT, 2, false), None);
        assert_eq!(state.key(0, RIGHT_ALT, 0, false), None);
    }
}
