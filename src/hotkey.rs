use crate::{
    config::HotkeyMode,
    engine::{Controller, Event},
};
use anyhow::{Context, Result};
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use std::sync::mpsc::Sender;

#[derive(Clone)]
pub struct Hotkey {
    commands: tokio::sync::mpsc::UnboundedSender<()>,
    mode: tokio::sync::watch::Sender<Option<HotkeyMode>>,
}

impl Hotkey {
    pub fn configure(&self) {
        let _ = self.commands.send(());
    }
    pub fn set_mode(&self, mode: HotkeyMode) {
        self.mode.send_if_modified(|current| {
            if *current == Some(mode) {
                return false;
            }
            *current = Some(mode);
            true
        });
    }
    pub fn shutdown(&self) {
        let _ = self.mode.send(None);
    }
}

pub fn spawn(controller: Controller, events: Sender<Event>, mode: HotkeyMode) -> Hotkey {
    let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
    let (mode, modes) = tokio::sync::watch::channel(Some(mode));
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(run(controller, &events, receiver, modes));
    });
    Hotkey { commands, mode }
}

async fn run(
    controller: Controller,
    events: &Sender<Event>,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<()>,
    mut modes: tokio::sync::watch::Receiver<Option<HotkeyMode>>,
) {
    loop {
        let Some(mode) = *modes.borrow_and_update() else {
            break;
        };
        while commands.try_recv().is_ok() {}
        let result = match mode {
            HotkeyMode::Portal => {
                run_portal(controller.clone(), events, &mut commands, &mut modes).await
            }
            HotkeyMode::RightAlt => {
                let locked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
                let lock_task = tokio::spawn(watch_screen_lock(
                    locked.clone(),
                    controller.clone(),
                    events.clone(),
                ));
                let c = controller.clone();
                let e = events.clone();
                let m = modes.clone();
                let result =
                    tokio::task::spawn_blocking(move || crate::evdev_hotkey::run(c, e, m, locked))
                        .await;
                lock_task.abort();
                match result {
                    Ok(result) => result,
                    Err(error) => Err(error.into()),
                }
            }
        };
        if let Err(error) = result {
            let _ = events.send(Event::Error(format!(
                "Global kısayol kullanılamıyor: {error:#}"
            )));
        }
        // A closed portal must not repeatedly prompt for a new shortcut.
        if *modes.borrow() == Some(mode) && modes.changed().await.is_err() {
            break;
        }
    }
}

async fn watch_screen_lock(
    locked: std::sync::Arc<std::sync::atomic::AtomicBool>,
    controller: Controller,
    events: Sender<Event>,
) {
    use ashpd::zbus;
    use std::sync::atomic::Ordering;
    let mut reported = false;
    loop {
        let result = async {
            let connection = zbus::Connection::session().await?;
            let proxy = zbus::Proxy::new(
                &connection,
                "org.gnome.ScreenSaver",
                "/org/gnome/ScreenSaver",
                "org.gnome.ScreenSaver",
            )
            .await?;
            let mut changes = proxy.receive_signal("ActiveChanged").await?;
            let active: bool = proxy.call("GetActive", &()).await?;
            locked.store(active, Ordering::SeqCst);
            while let Some(signal) = changes.next().await {
                let (active,): (bool,) = signal.body().deserialize()?;
                locked.store(active, Ordering::SeqCst);
                if active {
                    controller.cancel();
                }
            }
            Err::<(), zbus::Error>(zbus::Error::Failure(
                "Ekran kilidi bağlantısı kapandı".into(),
            ))
        }
        .await;
        locked.store(true, Ordering::SeqCst);
        controller.cancel();
        if !reported {
            let _ = events.send(Event::Error(format!(
                "Ekran kilidi durumu okunamadığı için AltGr devre dışı: {}",
                result.unwrap_err()
            )));
            reported = true;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

async fn run_portal(
    controller: Controller,
    events: &Sender<Event>,
    commands: &mut tokio::sync::mpsc::UnboundedReceiver<()>,
    modes: &mut tokio::sync::watch::Receiver<Option<HotkeyMode>>,
) -> Result<()> {
    ashpd::register_host_app(crate::APP_ID.parse()?)
        .await
        .context("Uygulama portal kimliği kaydedilemedi")?;
    let portal = GlobalShortcuts::new()
        .await
        .context("GlobalShortcuts portalı gerekli")?;
    let portal_supported = portal.version() >= 2;
    let _ = events.send(Event::ShortcutConfiguration { portal_supported });
    let session = portal.create_session(Default::default()).await?;
    // ashpd keeps Session::path private but serializes the handle as an object path.
    let session_path: String = serde_json::from_value(serde_json::to_value(&session)?)?;
    let mut closed = session.receive_closed().await?;
    let mut pressed = portal.receive_activated().await?;
    let mut released = portal.receive_deactivated().await?;
    let mut changed = portal.receive_shortcuts_changed().await?;
    let shortcuts = [
        NewShortcut::new("dictate", "Konuşarak yazmak için basılı tut")
            .preferred_trigger(Some("SUPER+SHIFT+space")),
    ];
    if *modes.borrow() != Some(HotkeyMode::Portal) {
        session.close().await?;
        return Ok(());
    }
    let bound = tokio::select! {
        result = portal.bind_shortcuts(&session, &shortcuts, None, Default::default()) => {
            match result.and_then(|request| request.response()) {
                Ok(bound) => bound,
                Err(error) => {
                    let _ = session.close().await;
                    return Err(error.into());
                }
            }
        }
        _ = modes.changed() => {
            session.close().await?;
            return Ok(());
        }
    };
    for shortcut in bound.shortcuts() {
        let _ = events.send(Event::Shortcut(shortcut.trigger_description().into()));
    }
    loop {
        tokio::select! {
            _ = modes.changed() => { break; }
            event = pressed.next() => {
                let Some(event) = event else { break; };
                if event.session_handle().as_str() == session_path && event.shortcut_id() == "dictate" { controller.start(); }
            }
            event = released.next() => {
                let Some(event) = event else { break; };
                if event.session_handle().as_str() == session_path && event.shortcut_id() == "dictate" { controller.stop(); }
            }
            event = changed.next() => {
                let Some(event) = event else { break; };
                if event.session_handle().as_str() == session_path {
                    for shortcut in event.shortcuts() { let _ = events.send(Event::Shortcut(shortcut.trigger_description().into())); }
                }
            }
            command = commands.recv() => {
                if command.is_none() { break; }
                if portal_supported {
                    if let Err(e) = portal.configure_shortcuts(&session, None, Default::default()).await {
                        let _ = events.send(Event::Error(format!("Kısayol ayarları açılamadı: {e}")));
                    }
                } else {
                    open_gnome_keyboard_settings(events.clone());
                }
            }
            _ = closed.next() => { break; }
        }
    }
    if controller.phase.load(std::sync::atomic::Ordering::SeqCst) == crate::engine::RECORDING {
        controller.cancel();
    }
    session.close().await?;
    Ok(())
}

fn open_gnome_keyboard_settings(events: Sender<Event>) {
    std::thread::spawn(move || {
        match std::process::Command::new("gnome-control-center")
            .arg("keyboard")
            .spawn()
        {
            Ok(mut child) => {
                let _ = events.send(Event::Status(
                    "GNOME Ayarlar'da Klavye → Kısayolları görüntüle ve özelleştir → Uygulama kısayolları bölümünden Just Whisper'ı seçin.".into(),
                ));
                match child.wait() {
                    Ok(status) if status.success() => {}
                    result => {
                        let _ = events.send(Event::Error(format!(
                            "GNOME klavye ayarları açılamadı: {result:?}"
                        )));
                    }
                }
            }
            Err(e) => {
                let _ = events.send(Event::Error(format!(
                    "Klavye ayarları açılamadı. GNOME Ayarlar → Klavye bölümünü açın: {e}"
                )));
            }
        }
    });
}
