use crate::{
    OBJECT_PATH, OVERLAY_IFACE,
    audio::{AudioBuffer, BANDS},
    autostart,
    config::{self, AsrBackend, Config, HotkeyMode},
    engine::{self, Event},
    hotkey,
    models::{self, MODELS},
};
use gtk::{
    cairo,
    glib::{self, variant::ToVariant},
    prelude::*,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

const CSS: &str = include_str!("style.css");
const RETENTION_SECONDS: &[u64] = &[0, 60, 300, 600, 1800];

fn model_index(path: &std::path::Path) -> u32 {
    MODELS
        .iter()
        .position(|spec| path.file_name() == spec.path().file_name())
        .map(|index| index as u32)
        .unwrap_or(MODELS.len() as u32)
}

fn model_selector(entry: &gtk::Entry) -> gtk::DropDown {
    let mut titles = MODELS.iter().map(|spec| spec.title).collect::<Vec<_>>();
    titles.push("Özel Whisper dosyası…");
    let choice = gtk::DropDown::from_strings(&titles);
    choice.set_selected(model_index(std::path::Path::new(entry.text().as_str())));
    let path = entry.clone();
    choice.connect_selected_notify(move |choice| {
        if let Some(spec) = MODELS.get(choice.selected() as usize) {
            let target = spec.path();
            if path.text().as_str() != target.to_string_lossy() {
                path.set_text(&target.to_string_lossy());
            }
        }
    });
    choice
}

fn active_model_text(path: &std::path::Path) -> String {
    let name = MODELS
        .iter()
        .find(|spec| path.file_name() == spec.path().file_name())
        .map(|spec| spec.title.to_owned())
        .unwrap_or_else(|| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    format!("Etkin model: {name}")
}

fn label(text: &str, class: &str) -> gtk::Label {
    let widget = gtk::Label::new(Some(text));
    widget.set_xalign(0.0);
    widget.add_css_class(class);
    widget
}

fn row(title: &str, description: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    row.add_css_class("setting-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 5);
    text.set_hexpand(true);
    text.append(&label(title, "row-title"));
    if !description.is_empty() {
        let detail = label(description, "muted");
        detail.set_wrap(true);
        detail.set_max_width_chars(40);
        text.append(&detail);
    }
    control
        .as_ref()
        .update_property(&[gtk::accessible::Property::Label(title)]);
    control.set_valign(gtk::Align::Center);
    row.append(&text);
    row.append(control);
    row
}

fn card(title: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("settings-card");
    card.append(&label(title, "card-title"));
    card
}

fn page(stack: &gtk::Stack, name: &str, title: &str) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
    content.add_css_class("settings-page");
    content.append(&label(title, "page-title"));
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .build();
    stack.add_named(&scroll, Some(name));
    content
}

const PAGES: &[(&str, &str, &str, &str)] = &[
    (
        "dictation",
        "Dikte",
        "input-keyboard-symbolic",
        "kısayol tuş altgr klavye yapıştırma çıktı",
    ),
    (
        "model",
        "Konuşma modeli",
        "audio-input-microphone-symbolic",
        "whisper model dosya indir dil türkçe english otomatik",
    ),
    (
        "audio",
        "Ses",
        "audio-volume-high-symbolic",
        "gürültü azaltma mikrofon deepfilternet",
    ),
    (
        "performance",
        "Performans",
        "power-profile-performance-symbolic",
        "gpu vulkan cpu bellek yükle hızlandırma aygıt",
    ),
    (
        "application",
        "Uygulama",
        "preferences-system-symbolic",
        "başlangıç sistem açılış arka plan gösterge overlay çıkış kapat",
    ),
    (
        "about",
        "Hakkında",
        "help-about-symbolic",
        "sürüm lisans yapılandırma dizin",
    ),
];

fn search_text(text: &str) -> String {
    text.replace('İ', "i").to_lowercase().replace('ı', "i")
}

struct SettingsUi {
    window: gtk::ApplicationWindow,
    #[cfg(test)]
    stack: gtk::Stack,
    #[cfg(test)]
    navigation: gtk::ListBox,
    #[cfg(test)]
    search: gtk::SearchEntry,
    drawing: gtk::DrawingArea,
    phase_label: gtk::Label,
    shortcut: gtk::Label,
    status: gtk::Label,
    backend_label: gtk::Label,
    active_model: gtk::Label,
    model_path: gtk::Entry,
    browse: gtk::Button,
    model_choice: gtk::DropDown,
    download: gtk::Button,
    cancel_download: gtk::Button,
    progress: gtk::ProgressBar,
    startup: gtk::Switch,
    lazy: gtk::Switch,
    retention: gtk::DropDown,
    noise_reduction: gtk::Switch,
    system_volume: gtk::Scale,
    gpu: gtk::Switch,
    overlay: gtk::Switch,
    language: gtk::DropDown,
    paste: gtk::DropDown,
    configure_key: gtk::Button,
    hotkey_mode: gtk::DropDown,
    save: gtk::Button,
    pending: gtk::Label,
    test: gtk::Button,
    cancel: gtk::Button,
    quit: gtk::Button,
}

fn settings_ui(app: &gtk::Application, initial: &Config, startup_enabled: bool) -> SettingsUi {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Just Whisper Ayarları")
        .default_width(1040)
        .default_height(760)
        .build();
    let header = gtk::HeaderBar::new();
    header.set_title_widget(Some(&label("Just Whisper Ayarları", "window-title")));
    window.set_titlebar(Some(&header));

    let shell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    window.set_child(Some(&shell));
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 18);
    sidebar.add_css_class("sidebar");
    sidebar.set_size_request(230, -1);
    shell.append(&sidebar);
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Ayarlarda ara")
        .build();
    search.update_property(&[gtk::accessible::Property::Label("Ayarlarda ara")]);
    sidebar.append(&search);

    let navigation = gtk::ListBox::new();
    navigation.add_css_class("navigation");
    navigation.set_selection_mode(gtk::SelectionMode::Single);
    let navigation_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&navigation)
        .build();
    sidebar.append(&navigation_scroll);
    for &(name, title, icon, _) in PAGES {
        let item = gtk::ListBoxRow::new();
        item.set_widget_name(name);
        item.update_property(&[gtk::accessible::Property::Label(title)]);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let image = gtk::Image::from_icon_name(icon);
        image.set_pixel_size(20);
        line.append(&image);
        line.append(&label(title, "nav-title"));
        item.set_child(Some(&line));
        navigation.append(&item);
    }

    let sidebar_status = gtk::Box::new(gtk::Orientation::Vertical, 8);
    sidebar_status.add_css_class("sidebar-status");
    let phase_label = label("Hazır", "phase-label");
    sidebar_status.append(&phase_label);
    let drawing = gtk::DrawingArea::builder()
        .content_width(190)
        .content_height(56)
        .build();
    drawing.set_visible(false);
    sidebar_status.append(&drawing);
    let shortcut = label("Kısayol bekleniyor…", "shortcut");
    shortcut.set_wrap(true);
    shortcut.set_max_width_chars(23);
    sidebar_status.append(&shortcut);
    let test = gtk::Button::with_label("Test için basılı tut");
    sidebar_status.append(&test);
    let cancel = gtk::Button::with_label("Kaydı iptal et");
    cancel.set_visible(false);
    sidebar_status.append(&cancel);
    sidebar.append(&sidebar_status);
    let close = gtk::Button::with_label("Pencereyi kapat");
    close.set_tooltip_text(Some("Uygulama arka planda çalışmaya devam eder."));
    let w = window.clone();
    close.connect_clicked(move |_| w.close());
    sidebar.append(&close);

    let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.set_hexpand(true);
    shell.append(&main);
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.set_hhomogeneous(false);
    stack.set_vhomogeneous(false);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(120);
    main.append(&stack);

    let dictation = page(&stack, "dictation", "Dikte");
    let shortcuts = card("Klavye kısayolu");
    let hotkey_mode = gtk::DropDown::from_strings(&["Sağ AltGr", "Masaüstü kısayolu"]);
    hotkey_mode.set_selected(u32::from(initial.hotkey_mode == HotkeyMode::Portal));
    shortcuts.append(&row(
        "Konuşma tuşu",
        "Kayıt için basılı tutun, bitirmek için bırakın.",
        &hotkey_mode,
    ));
    let configure_key = gtk::Button::with_label("Kısayolu değiştir");
    configure_key.set_sensitive(false);
    let portal_settings = row(
        "Masaüstü kısayolu",
        "Varsayılan: Super + Shift + Space",
        &configure_key,
    );
    portal_settings.set_visible(initial.hotkey_mode == HotkeyMode::Portal);
    let portal_row = portal_settings.clone();
    hotkey_mode
        .connect_selected_notify(move |choice| portal_row.set_visible(choice.selected() == 1));
    shortcuts.append(&portal_settings);
    dictation.append(&shortcuts);
    let hint = label(
        "AltGr ile başka bir tuşa basmak kaydı iptal eder.",
        "info-note",
    );
    hint.set_wrap(true);
    hint.set_visible(initial.hotkey_mode == HotkeyMode::RightAlt);
    let altgr_hint = hint.clone();
    hotkey_mode
        .connect_selected_notify(move |choice| altgr_hint.set_visible(choice.selected() == 0));
    dictation.append(&hint);
    let output = card("Metin çıktısı");
    let paste = gtk::DropDown::from_strings(&["Ctrl + V", "Ctrl + Shift + V"]);
    paste.set_selected(u32::from(initial.paste_shortcut == "ctrl-shift-v"));
    output.append(&row(
        "Yapıştırma kısayolu",
        "Terminal için Ctrl + Shift + V seçin.",
        &paste,
    ));
    dictation.append(&output);

    let model = page(&stack, "model", "Konuşma modeli");
    let model_card = card("Konuşma modeli");
    let model_path = gtk::Entry::builder()
        .text(initial.model_path.to_string_lossy())
        .hexpand(true)
        .placeholder_text("GGML model dosyasının yolu")
        .build();
    model_path.update_property(&[gtk::accessible::Property::Label("Model dosyası")]);
    let model_choice = model_selector(&model_path);
    let download = gtk::Button::with_label("Tekrar dene");
    download.set_visible(false);
    let cancel_download = gtk::Button::with_label("Durdur");
    cancel_download.set_visible(false);
    let downloads = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    downloads.append(&model_choice);
    downloads.append(&download);
    downloads.append(&cancel_download);
    model_card.append(&row(
        "Model",
        "Seçilen model otomatik indirilir ve etkinleşir.",
        &downloads,
    ));
    let model_note = label(
        "Qwen ilk seçimde yerel çalışma ortamını da kurar. İndirmeden sonra çevrimdışı çalışır.",
        "info-note",
    );
    model_note.set_wrap(true);
    model_note.set_max_width_chars(60);

    let file_row = gtk::Box::new(gtk::Orientation::Vertical, 10);
    file_row.add_css_class("setting-row");
    file_row.append(&label("Model dosyası", "row-title"));
    let model_controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let browse = gtk::Button::with_label("Dosya seç…");
    model_controls.append(&model_path);
    model_controls.append(&browse);
    file_row.append(&model_controls);
    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    progress.set_visible(false);
    model_card.append(&progress);
    let custom = gtk::Expander::builder()
        .label("Özel Whisper modeli")
        .child(&file_row)
        .build();
    custom.set_visible(model_choice.selected() == MODELS.len() as u32);
    custom.set_expanded(true);
    let custom_row = custom.clone();
    model_choice.connect_selected_notify(move |choice| {
        custom_row.set_visible(choice.selected() == MODELS.len() as u32);
    });
    model_card.append(&custom);
    let active_model = label(&active_model_text(&initial.model_path), "muted");
    active_model.set_wrap(true);
    active_model.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    active_model.set_max_width_chars(60);
    active_model.set_margin_start(20);
    active_model.set_margin_end(20);
    active_model.set_margin_top(12);
    active_model.set_margin_bottom(16);
    model_card.append(&active_model);
    model.append(&model_card);
    model.append(&model_note);
    let language_card = card("Dil");
    let language = gtk::DropDown::from_strings(&["Türkçe", "Otomatik", "English"]);
    language.set_selected(match initial.language.as_str() {
        "auto" => 1,
        "en" => 2,
        _ => 0,
    });
    language_card.append(&row(
        "Konuşma dili",
        ".en modelleri yalnızca İngilizceyi destekler.",
        &language,
    ));
    model.append(&language_card);

    let audio = page(&stack, "audio", "Ses");
    let audio_card = card("Ses işleme");
    let noise_reduction = gtk::Switch::builder()
        .active(initial.noise_reduction)
        .build();
    audio_card.append(&row(
        "Gürültü azaltma",
        "DeepFilterNet3 ile arka plan gürültüsünü azaltır.",
        &noise_reduction,
    ));
    let system_volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    system_volume.set_value(f64::from(initial.system_volume_percent));
    system_volume.set_draw_value(false);
    system_volume.set_hexpand(true);
    system_volume.set_size_request(180, -1);
    let system_volume_value = label("", "muted");
    system_volume_value.set_width_chars(7);
    let volume_control = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    volume_control.append(&system_volume);
    volume_control.append(&system_volume_value);
    let update_volume_label = {
        let value_label = system_volume_value.clone();
        move |volume: f64| {
            let text = if volume.round() == 0.0 {
                "Kapalı".into()
            } else {
                format!("{}%", volume.round() as u8)
            };
            value_label.set_text(&text);
        }
    };
    update_volume_label(system_volume.value());
    system_volume.connect_value_changed(move |scale| update_volume_label(scale.value()));
    audio_card.append(&row(
        "Tuşa basınca sistem sesi",
        "%100 mevcut düzeyi korur, %0 sesi kapatır. Tuşu bırakınca önceki düzey geri yüklenir.",
        &volume_control,
    ));
    audio.append(&audio_card);

    let performance = page(&stack, "performance", "Performans");
    let compute = card("Hesaplama");
    let gpu = gtk::Switch::builder().active(initial.use_gpu).build();
    compute.append(&row(
        "GPU hızlandırma",
        "Whisper Vulkan, Qwen CUDA kullanır. GPU bulunamazsa CPU ile çalışır.",
        &gpu,
    ));
    let backend_label = label("Model henüz yüklenmedi", "muted");
    backend_label.set_wrap(true);
    backend_label.set_max_width_chars(36);
    compute.append(&row("Kullanılan aygıt", "", &backend_label));
    performance.append(&compute);
    let memory = card("Bellek");
    let lazy = gtk::Switch::builder().active(initial.load_on_press).build();
    memory.append(&row(
        "Modeli tuşa basınca yükle",
        "Açıkken ilk kullanımda yükler. Kapalıyken önceden yükler ve sürekli bellekte tutar.",
        &lazy,
    ));
    let retention = gtk::DropDown::from_strings(&[
        "Hemen boşalt",
        "1 dakika",
        "5 dakika",
        "10 dakika",
        "30 dakika",
    ]);
    let mut durations = RETENTION_SECONDS.to_vec();
    if !durations.contains(&initial.model_keep_alive_secs) {
        durations.push(initial.model_keep_alive_secs);
        let list = retention
            .model()
            .unwrap()
            .downcast::<gtk::StringList>()
            .unwrap();
        list.append(&format!("{} saniye", initial.model_keep_alive_secs));
    }
    retention.set_selected(
        durations
            .iter()
            .position(|seconds| *seconds == initial.model_keep_alive_secs)
            .unwrap() as u32,
    );
    retention.set_sensitive(initial.load_on_press);
    let duration_control = retention.clone();
    lazy.connect_active_notify(move |toggle| duration_control.set_sensitive(toggle.is_active()));
    memory.append(&row("Modeli bellekte tutma süresi", "Son kullanımdan sonra bu süre boyunca hazır kalır. GPU kullanırken VRAM, CPU kullanırken RAM ayrılır.", &retention));
    performance.append(&memory);

    let application = page(&stack, "application", "Uygulama");
    let startup_card = card("Başlangıç");
    let startup = gtk::Switch::builder().active(startup_enabled).build();
    startup_card.append(&row(
        "Sistem açılışında başlat",
        "Oturum açıldığında arka planda çalışır.",
        &startup,
    ));
    application.append(&startup_card);
    let appearance = card("Gösterge");
    let overlay = gtk::Switch::builder().active(initial.overlay).build();
    appearance.append(&row(
        "Konuşma göstergesi",
        "Kayıt ve işleme durumunu gösterir. GNOME eklentisi gerektirir.",
        &overlay,
    ));
    application.append(&appearance);
    let session = card("Oturum");
    let quit = gtk::Button::with_label("Uygulamadan çık");
    session.append(&row(
        "Uygulamayı kapat",
        "Arka plan işlemini ve global kısayolu durdurur.",
        &quit,
    ));
    application.append(&session);

    let about = page(&stack, "about", "Hakkında");
    let details = card("Just Whisper");
    details.append(&row(
        "Sürüm",
        "",
        &label(env!("CARGO_PKG_VERSION"), "muted"),
    ));
    details.append(&row("Lisans", "", &label("MIT", "muted")));
    let paths = gtk::Box::new(gtk::Orientation::Vertical, 8);
    paths.add_css_class("setting-row");
    for (title, path) in [
        ("Ayar dosyası", config::config_path()),
        ("Model dizini", config::data_dir().join("models")),
    ] {
        paths.append(&label(title, "row-title"));
        let path_label = label(&path.to_string_lossy(), "muted");
        path_label.set_selectable(true);
        path_label.set_wrap(true);
        path_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        path_label.set_max_width_chars(60);
        paths.append(&path_label);
    }
    details.append(&paths);
    about.append(&details);

    let empty = gtk::Box::new(gtk::Orientation::Vertical, 12);
    empty.set_valign(gtk::Align::Center);
    empty.set_halign(gtk::Align::Center);
    empty.append(&gtk::Image::from_icon_name("edit-find-symbolic"));
    empty.append(&label("Ayar bulunamadı", "row-title"));
    stack.add_named(&empty, Some("empty"));

    let pages = stack.clone();
    navigation.connect_row_selected(move |_, item| {
        if let Some(item) = item {
            pages.set_visible_child_name(&item.widget_name());
        }
    });
    navigation.select_row(navigation.row_at_index(0).as_ref());
    let list = navigation.clone();
    let pages = stack.clone();
    search.connect_search_changed(move |search| {
        let query = search_text(search.text().trim());
        let mut first_match = None;
        for (index, &(_, title, _, keywords)) in PAGES.iter().enumerate() {
            let item = list.row_at_index(index as i32).unwrap();
            let matches = query
                .split_whitespace()
                .all(|word| search_text(&format!("{title} {keywords}")).contains(word));
            item.set_visible(matches);
            if matches && first_match.is_none() {
                first_match = Some(item);
            }
        }
        if list.selected_row().is_none_or(|item| !item.is_visible()) {
            list.select_row(first_match.as_ref());
        }
        if first_match.is_none() {
            pages.set_visible_child_name("empty");
        }
    });

    let footer = gtk::Box::new(gtk::Orientation::Vertical, 10);
    footer.add_css_class("footer");
    let status = label("", "status");
    status.set_wrap(true);
    status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status.set_max_width_chars(65);
    status.set_lines(3);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.set_selectable(true);
    status.connect_label_notify(|status| status.set_tooltip_text(Some(&status.text())));
    footer.append(&status);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let pending = label("", "muted");
    pending.set_hexpand(true);
    pending.set_wrap(true);
    actions.append(&pending);
    let save = gtk::Button::with_label("Ayarları kaydet");
    save.add_css_class("suggested-action");
    actions.append(&save);
    footer.append(&actions);
    main.append(&footer);

    // Keep edits in the same widgets when switching pages. Save applies all pages.
    for choice in [&language, &paste, &hotkey_mode, &retention] {
        let dirty = pending.clone();
        choice.connect_selected_notify(move |_| dirty.set_text("Kaydedilmemiş değişiklikler"));
    }
    for toggle in [&startup, &lazy, &noise_reduction, &gpu, &overlay] {
        let dirty = pending.clone();
        toggle.connect_active_notify(move |_| dirty.set_text("Kaydedilmemiş değişiklikler"));
    }
    {
        let dirty = pending.clone();
        system_volume.connect_value_changed(move |_| dirty.set_text("Kaydedilmemiş değişiklikler"));
    }

    SettingsUi {
        window,
        #[cfg(test)]
        stack,
        #[cfg(test)]
        navigation,
        #[cfg(test)]
        search,
        drawing,
        phase_label,
        shortcut,
        status,
        backend_label,
        active_model,
        model_path,
        browse,
        model_choice,
        download,
        cancel_download,
        progress,
        startup,
        lazy,
        retention,
        noise_reduction,
        system_volume,
        gpu,
        overlay,
        language,
        paste,
        configure_key,
        hotkey_mode,
        save,
        pending,
        test,
        cancel,
        quit,
    }
}

enum ModelDownloadEvent {
    Progress(models::Progress),
    Finished(Result<std::path::PathBuf, String>),
}

struct ModelDownload {
    receiver: mpsc::Receiver<ModelDownloadEvent>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    index: u32,
}

struct ModelManager {
    download_job: Rc<RefCell<Option<ModelDownload>>>,
    pending_model: Rc<RefCell<Option<(std::path::PathBuf, AsrBackend)>>>,
    switching: Rc<Cell<bool>>,
    failed_model: Rc<Cell<Option<u32>>>,
    model_choice: gtk::DropDown,
    model_path: gtk::Entry,
    download: gtk::Button,
    cancel_download: gtk::Button,
    progress: gtk::ProgressBar,
    active_model: gtk::Label,
    status: gtk::Label,
}

impl ModelManager {
    fn new(ui: &SettingsUi) -> Self {
        Self::new_with(ui, |spec, cancel, sender| {
            models::prepare(spec, cancel, |progress| {
                let _ = sender.send(ModelDownloadEvent::Progress(progress));
            })
            .map_err(|error| format!("{error:#}"))
        })
    }

    fn new_with(
        ui: &SettingsUi,
        prepare: impl Fn(
            &'static models::ModelSpec,
            &std::sync::atomic::AtomicBool,
            &mpsc::Sender<ModelDownloadEvent>,
        ) -> Result<std::path::PathBuf, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let window = ui.window.clone();
        let model_choice = ui.model_choice.clone();
        let model_path = ui.model_path.clone();
        let download = ui.download.clone();
        let cancel_download = ui.cancel_download.clone();
        let progress = ui.progress.clone();
        let active_model = ui.active_model.clone();
        let status = ui.status.clone();
        let browse = ui.browse.clone();
        let download_job = Rc::new(RefCell::new(None::<ModelDownload>));
        let pending_model = Rc::new(RefCell::new(None::<(std::path::PathBuf, AsrBackend)>));
        let switching = Rc::new(Cell::new(false));
        let failed_model = Rc::new(Cell::new(None::<u32>));
        let job = download_job.clone();
        let queued = pending_model.clone();
        let guard = switching.clone();
        let p = progress.clone();
        let retry = download.clone();
        let stop_button = cancel_download.clone();
        let status_label = status.clone();
        let failed = failed_model.clone();
        let prepare = Arc::new(prepare);
        model_choice.connect_selected_notify(move |choice| {
            if guard.get() {
                return;
            }
            failed.set(None);
            let Some(spec) = MODELS.get(choice.selected() as usize) else {
                return;
            };
            if let Some(previous) = job.borrow_mut().take() {
                previous.cancel.store(true, Ordering::SeqCst);
            }
            queued.borrow_mut().take();
            retry.set_visible(false);
            choice.set_sensitive(false);
            stop_button.set_visible(true);
            p.set_visible(true);
            p.set_text(Some("Model hazırlanıyor…"));
            status_label
                .set_text("Seçilen model hazırlanıyor. Hazır olunca otomatik etkinleşecek.");
            let (sender, receiver) = mpsc::channel();
            let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = cancel.clone();
            *job.borrow_mut() = Some(ModelDownload {
                receiver,
                cancel,
                index: choice.selected(),
            });
            let prepare = prepare.clone();
            std::thread::spawn(move || {
                let result = prepare(spec, &flag, &sender);
                let _ = sender.send(ModelDownloadEvent::Finished(result));
            });
        });
        let job = download_job.clone();
        cancel_download.connect_clicked(move |_| {
            if let Some(job) = job.borrow().as_ref() {
                job.cancel.store(true, Ordering::SeqCst);
            }
        });
        let choice = model_choice.clone();
        let failed = failed_model.clone();
        download.connect_clicked(move |_| {
            if let Some(index) = failed.get() {
                if choice.selected() == index {
                    choice.notify("selected");
                } else {
                    choice.set_selected(index);
                }
            }
        });

        let queued = pending_model.clone();
        model_path.connect_activate(move |entry| {
            *queued.borrow_mut() = Some((entry.text().as_str().into(), AsrBackend::Whisper));
        });
        let w = window.clone();
        let entry = model_path.clone();
        let queued = pending_model.clone();
        browse.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Whisper modeli seç")
                .build();
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Whisper modelleri (*.bin)"));
            filter.add_pattern("*.bin");
            let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            let entry = entry.clone();
            let queued = queued.clone();
            dialog.open(Some(&w), None::<&gtk::gio::Cancellable>, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    entry.set_text(&path.to_string_lossy());
                    *queued.borrow_mut() = Some((path, AsrBackend::Whisper));
                }
            });
        });

        Self {
            download_job,
            pending_model,
            switching,
            failed_model,
            model_choice,
            model_path,
            download,
            cancel_download,
            progress,
            active_model,
            status,
        }
    }

    fn poll(
        &self,
        saved: &Config,
        idle: bool,
        persist: impl Fn(&Config) -> anyhow::Result<()>,
    ) -> Option<Config> {
        let mut activated = None;
        let mut completed = None;
        if let Some(job) = self.download_job.borrow().as_ref() {
            while let Ok(event) = job.receiver.try_recv() {
                match event {
                    ModelDownloadEvent::Progress(update) => {
                        self.progress.set_text(Some(&update.text));
                        match update.fraction {
                            Some(fraction) => self.progress.set_fraction(fraction),
                            None => self.progress.pulse(),
                        }
                    }
                    ModelDownloadEvent::Finished(result) => completed = Some((job.index, result)),
                }
            }
        }
        if let Some((index, result)) = completed {
            self.download_job.borrow_mut().take();
            self.cancel_download.set_visible(false);
            self.progress.set_visible(false);
            match result {
                Ok(path) => {
                    *self.pending_model.borrow_mut() = Some((path, MODELS[index as usize].backend));
                    self.status
                        .set_text("Model hazır. Kayıt varsa bittikten sonra etkinleşecek.");
                }
                Err(error) => {
                    self.model_choice.set_sensitive(true);
                    self.status.set_text(&error);
                    self.status.add_css_class("error");
                    self.download.set_visible(true);
                    self.failed_model.set(Some(index));
                    self.switching.set(true);
                    self.model_choice
                        .set_selected(model_index(&saved.model_path));
                    self.model_path
                        .set_text(&saved.model_path.to_string_lossy());
                    self.switching.set(false);
                }
            }
        }
        if idle && let Some((path, backend)) = self.pending_model.borrow_mut().take() {
            let mut next = saved.clone();
            next.model_path = path;
            next.asr_backend = backend;
            match next.validate().and_then(|_| persist(&next)) {
                Ok(()) => {
                    self.active_model
                        .set_text(&active_model_text(&next.model_path));
                    activated = Some(next);
                    self.status.set_text("Seçilen model etkinleştirildi.");
                    self.status.remove_css_class("error");
                    self.download.set_visible(false);
                }
                Err(error) => {
                    self.status.set_text(&format!("{error:#}"));
                    self.status.add_css_class("error");
                    self.switching.set(true);
                    self.model_choice
                        .set_selected(model_index(&saved.model_path));
                    self.model_path
                        .set_text(&saved.model_path.to_string_lossy());
                    self.switching.set(false);
                }
            }
            self.model_choice.set_sensitive(true);
        }
        if self
            .download_job
            .borrow()
            .as_ref()
            .is_some_and(|job| MODELS[job.index as usize].backend == AsrBackend::Qwen3)
        {
            self.progress.pulse();
        }
        activated
    }
}

#[derive(Default)]
struct Visual {
    phase: u8,
    levels: Vec<f64>,
    audio: Option<Arc<Mutex<AudioBuffer>>>,
}

fn draw(cr: &cairo::Context, w: f64, h: f64, phase: u8, levels: &[f64], time: f64) {
    let width = if phase == engine::PROCESSING {
        48.0
    } else {
        172.0
    };
    let height = 48.0;
    let x = (w - width) / 2.0;
    let y = (h - height) / 2.0;
    cr.set_source_rgb(0.02, 0.02, 0.025);
    let radius = height / 2.0;
    cr.arc(
        x + width - radius,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        std::f64::consts::FRAC_PI_2,
    );
    cr.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
    let _ = cr.fill_preserve();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_cap(cairo::LineCap::Round);
    if phase == engine::PROCESSING {
        cr.set_line_width(2.5);
        cr.arc(w / 2.0, h / 2.0, 9.0, time * 4.0, time * 4.0 + 4.3);
        let _ = cr.stroke();
    } else {
        cr.set_line_width(2.5);
        for i in 0..BANDS {
            let amplitude = levels.get(i).copied().unwrap_or(0.0);
            let height = 3.0 + amplitude * 25.0;
            let x = w / 2.0 + (i as f64 - 10.0) * 6.0;
            cr.move_to(x, h / 2.0 - height / 2.0);
            cr.line_to(x, h / 2.0 + height / 2.0);
            let _ = cr.stroke();
        }
    }
}

pub fn build(app: &gtk::Application, background: bool) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(false);
    }
    let (events, receiver) = mpsc::channel();
    let (initial, mut config_error) = match Config::load() {
        Ok(config) => (config, None),
        Err(e) => (Config::default(), Some(format!("{e:#}"))),
    };
    let saved = Rc::new(RefCell::new(initial.clone()));
    let controller = engine::spawn(initial.clone(), events.clone());
    let hotkey = hotkey::spawn(controller.clone(), events.clone(), initial.hotkey_mode);
    let paste_events = events.clone();
    std::thread::spawn(move || {
        if let Err(error) = crate::output::start_paste_service() {
            let _ = paste_events.send(Event::Error(format!("{error:#}")));
        }
    });
    let visual = Rc::new(RefCell::new(Visual::default()));

    let startup_enabled = match autostart::is_enabled() {
        Ok(enabled) => enabled,
        Err(error) => {
            let message = format!("{error:#}");
            config_error = Some(match config_error {
                Some(previous) => format!("{previous}\n{message}"),
                None => message,
            });
            false
        }
    };
    let ui = settings_ui(app, &initial, startup_enabled);
    let model_manager = ModelManager::new(&ui);
    let SettingsUi {
        window,
        drawing,
        phase_label,
        shortcut,
        status,
        backend_label,
        startup,
        lazy,
        retention,
        noise_reduction,
        system_volume,
        gpu,
        overlay,
        language,
        paste,
        configure_key,
        hotkey_mode,
        save,
        pending,
        test,
        cancel,
        quit,
        ..
    } = ui;
    let start_time = Instant::now();
    let v = visual.clone();
    drawing.set_draw_func(move |_, cr, w, h| {
        let v = v.borrow();
        draw(
            cr,
            w as f64,
            h as f64,
            v.phase,
            &v.levels,
            start_time.elapsed().as_secs_f64(),
        );
    });

    let gesture = gtk::GestureClick::new();
    let c = controller.clone();
    gesture.connect_pressed(move |_, _, _, _| c.start());
    let c = controller.clone();
    gesture.connect_released(move |_, _, _, _| c.stop());
    let c = controller.clone();
    gesture.connect_cancel(move |_, _| c.stop());
    test.add_controller(gesture);
    let c = controller.clone();
    cancel.connect_clicked(move |_| c.cancel());
    let a = app.clone();
    quit.connect_clicked(move |_| a.quit());
    let h = hotkey.clone();
    configure_key.connect_clicked(move |_| h.configure());

    let c = controller.clone();
    let s = saved.clone();
    let status_label = status.clone();
    let h = hotkey.clone();
    let initial_retention = initial.model_keep_alive_secs;
    save.connect_clicked(move |_| {
        if c.phase.load(Ordering::SeqCst) != engine::IDLE {
            status_label.set_text("Ayarları kaydetmek için kaydın bitmesini bekleyin.");
            return;
        }
        let config = Config {
            asr_backend: s.borrow().asr_backend,
            model_path: s.borrow().model_path.clone(),
            language: ["tr", "auto", "en"][language.selected() as usize].into(),
            load_on_press: lazy.is_active(),
            model_keep_alive_secs: RETENTION_SECONDS
                .get(retention.selected() as usize)
                .copied()
                .unwrap_or(initial_retention),
            overlay: overlay.is_active(),
            paste_shortcut: ["ctrl-v", "ctrl-shift-v"][paste.selected() as usize].into(),
            use_gpu: gpu.is_active(),
            noise_reduction: noise_reduction.is_active(),
            system_volume_percent: system_volume.value().round() as u8,
            hotkey_mode: if hotkey_mode.selected() == 0 {
                HotkeyMode::RightAlt
            } else {
                HotkeyMode::Portal
            },
        };
        match config.validate().and_then(|_| config.save()) {
            Ok(()) => {
                *s.borrow_mut() = config.clone();
                h.set_mode(config.hotkey_mode);
                c.configure(config);
                match autostart::set_enabled(startup.is_active()) {
                    Ok(()) => {
                        pending.set_text("");
                        status_label.set_text("Ayarlar kaydedildi.");
                        status_label.remove_css_class("error");
                    }
                    Err(error) => {
                        status_label.set_text(&format!("Diğer ayarlar kaydedildi. {error:#}"));
                        status_label.add_css_class("error");
                    }
                }
            }
            Err(e) => {
                status_label.set_text(&format!("{e:#}"));
                status_label.add_css_class("error");
            }
        }
    });

    // The application stays resident while its settings window is hidden.
    let hold = Rc::new(RefCell::new(Some(app.hold())));
    window.connect_close_request(|window| {
        window.set_visible(false);
        glib::Propagation::Stop
    });
    let c = controller.clone();
    let keepalive = hold.clone();
    let h = hotkey.clone();
    let job = model_manager.download_job.clone();
    app.connect_shutdown(move |_| {
        if let Some(job) = job.borrow().as_ref() {
            job.cancel.store(true, Ordering::SeqCst);
        }
        h.shutdown();
        c.shutdown();
        keepalive.borrow_mut().take();
    });

    let bus = app.dbus_connection();
    let tick = Cell::new(0u64);
    let error = config_error;
    if let Some(e) = error {
        status.set_text(&e);
        status.add_css_class("error");
    }
    if !initial.model_path.exists()
        || (initial.asr_backend == AsrBackend::Qwen3
            && (!initial.model_path.join(".ready").is_file() || !crate::qwen::python().is_file()))
    {
        model_manager.model_choice.notify("selected");
    }
    let displayed_phase = Cell::new(u8::MAX);
    glib::timeout_add_local(Duration::from_millis(33), move || {
        while let Ok(event) = receiver.try_recv() {
            match event {
                Event::Recording(audio) => {
                    let mut v = visual.borrow_mut();
                    v.phase = engine::RECORDING;
                    v.audio = Some(audio);
                    status.set_text("Kaydediliyor…");
                    status.remove_css_class("error");
                }
                Event::Processing => {
                    let mut v = visual.borrow_mut();
                    v.phase = engine::PROCESSING;
                    v.audio = None;
                    status.set_text("Konuşma metne çevriliyor…");
                }
                Event::Done(text) => {
                    let mut v = visual.borrow_mut();
                    v.phase = engine::IDLE;
                    v.audio = None;
                    v.levels.clear();
                    status.set_text(&text);
                    status.remove_css_class("error");
                }
                Event::Error(error) => {
                    if controller.phase.load(Ordering::SeqCst) == engine::IDLE {
                        let mut v = visual.borrow_mut();
                        v.phase = engine::IDLE;
                        v.audio = None;
                        v.levels.clear();
                    }
                    status.set_text(&error);
                    status.add_css_class("error");
                }
                Event::Status(text) => {
                    if visual.borrow().phase != engine::RECORDING {
                        status.set_text(&text);
                        status.remove_css_class("error");
                    }
                }
                Event::Shortcut(text) => shortcut.set_text(&text),
                Event::ShortcutConfiguration { portal_supported } => {
                    configure_key.set_sensitive(true);
                    configure_key.set_label(if portal_supported {
                        "Kısayolu değiştir"
                    } else {
                        "GNOME ayarlarını aç"
                    });
                    configure_key.set_tooltip_text(Some(if portal_supported {
                        "Masaüstünün kısayol yapılandırma penceresini aç."
                    } else {
                        "GNOME klavye ayarlarından Just Whisper kısayolunu değiştir."
                    }));
                }
                Event::Backend(text) => backend_label.set_text(&text),
            }
        }
        let current = saved.borrow().clone();
        if let Some(next) = model_manager.poll(
            &current,
            controller.phase.load(Ordering::SeqCst) == engine::IDLE,
            Config::save,
        ) {
            controller.configure(next.clone());
            *saved.borrow_mut() = next;
            backend_label.set_text("Model henüz yüklenmedi");
        }
        let mut v = visual.borrow_mut();
        if let Some(audio) = &v.audio {
            let data = audio.lock().unwrap();
            let levels = data.levels.clone();
            if data.finished && controller.phase.load(Ordering::SeqCst) == engine::RECORDING {
                controller.stop();
            }
            drop(data);
            v.levels = levels;
        }
        if displayed_phase.get() != v.phase {
            displayed_phase.set(v.phase);
            let busy = v.phase != engine::IDLE;
            phase_label.set_text(match v.phase {
                engine::RECORDING => "Kaydediliyor",
                engine::PROCESSING => "İşleniyor",
                _ => "Hazır",
            });
            phase_label.remove_css_class("recording");
            phase_label.remove_css_class("processing");
            if v.phase == engine::RECORDING {
                phase_label.add_css_class("recording");
            }
            if v.phase == engine::PROCESSING {
                phase_label.add_css_class("processing");
            }
            drawing.set_visible(busy);
            cancel.set_visible(busy);
            save.set_sensitive(!busy);
            test.set_sensitive(v.phase != engine::PROCESSING);
        }
        if v.phase != engine::IDLE {
            drawing.queue_draw();
        }
        tick.set(tick.get() + 1);
        if v.phase != engine::IDLE || tick.get().is_multiple_of(30) {
            let state = if !saved.borrow().overlay {
                "hidden"
            } else {
                match v.phase {
                    engine::RECORDING => "recording",
                    engine::PROCESSING => "processing",
                    _ => "hidden",
                }
            };
            if let Some(bus) = &bus {
                let _ = bus.emit_signal(
                    None,
                    OBJECT_PATH,
                    OVERLAY_IFACE,
                    "Update",
                    Some(&(state, v.levels.clone()).to_variant()),
                );
            }
        }
        glib::ControlFlow::Continue
    });
    if !background {
        window.present();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle() {
        let deadline = Instant::now() + Duration::from_millis(350);
        let context = glib::MainContext::default();
        while Instant::now() < deadline {
            while context.pending() {
                context.iteration(false);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn capture(ui: &SettingsUi, name: &str) {
        let Some(directory) = std::env::var_os("WHISPER_UI_SCREENSHOTS") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let paintable = gtk::WidgetPaintable::new(Some(&ui.window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(
            &snapshot,
            ui.window.width() as f64,
            ui.window.height() as f64,
        );
        let node = snapshot.to_node().unwrap();
        let texture = ui.window.renderer().unwrap().render_texture(&node, None);
        texture
            .save_to_png(directory.join(format!("{name}.png")))
            .unwrap();
    }

    fn check_automatic_model_selection(app: &gtk::Application) {
        let directory = std::env::temp_dir().join(format!(
            "just-whisper-selection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let whisper = directory.join("ggml-small.bin");
        std::fs::write(&whisper, b"model fixture").unwrap();
        let qwen = directory.join("qwen3-asr-1.7b");
        std::fs::create_dir_all(&qwen).unwrap();
        for name in [".ready", "config.json", "model.safetensors.index.json"] {
            std::fs::write(qwen.join(name), b"fixture").unwrap();
        }
        let mut saved = Config {
            noise_reduction: false,
            hotkey_mode: HotkeyMode::RightAlt,
            ..Config::default()
        };
        let ui = settings_ui(app, &saved, false);
        let manager = ModelManager::new_with(&ui, move |spec, cancel, sender| {
            let _ = sender.send(ModelDownloadEvent::Progress(models::Progress {
                text: "İndiriliyor…".into(),
                fraction: Some(0.5),
            }));
            match spec.id {
                "small" => Ok(whisper.clone()),
                "qwen3-asr-1.7b" => Ok(qwen.clone()),
                "medium" => {
                    while !cancel.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err("İndirme durduruldu".into())
                }
                _ => Err("İndirme başarısız".into()),
            }
        });
        let wait_for_download = |saved: &Config| {
            let deadline = Instant::now() + Duration::from_secs(3);
            while manager.download_job.borrow().is_some() {
                assert!(Instant::now() < deadline);
                assert!(
                    manager
                        .poll(saved, false, |_| panic!(
                            "Kayıt sürerken model uygulanmamalı"
                        ))
                        .is_none()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        // Selection starts preparation without a download or save click. Delay
        // activation until recording finishes, preserving unrelated settings.
        ui.model_choice.set_selected(2);
        assert!(!ui.model_choice.is_sensitive());
        assert!(ui.progress.property::<bool>("visible"));
        wait_for_download(&saved);
        assert_eq!(saved.model_path, Config::default().model_path);
        let persists = Cell::new(0);
        saved = manager
            .poll(&saved, true, |_| {
                persists.set(persists.get() + 1);
                Ok(())
            })
            .unwrap();
        assert_eq!(persists.get(), 1);
        assert_eq!(saved.asr_backend, AsrBackend::Whisper);
        assert!(!saved.noise_reduction);
        assert_eq!(saved.hotkey_mode, HotkeyMode::RightAlt);
        assert_eq!(ui.model_choice.selected(), 2);
        assert!(ui.model_choice.is_sensitive());
        assert!(ui.pending.text().is_empty());
        assert_eq!(ui.retention.selected(), 2);
        assert!(ui.retention.is_sensitive());
        ui.lazy.set_active(false);
        assert!(!ui.retention.is_sensitive());
        ui.lazy.set_active(true);
        assert!(ui.retention.is_sensitive());
        ui.retention.set_selected(3);
        assert_eq!(ui.pending.text().as_str(), "Kaydedilmemiş değişiklikler");
        ui.pending.set_text("");
        let active = ui.active_model.text();

        // Failed and cancelled downloads retain the previous selected model.
        ui.model_choice.set_selected(0);
        wait_for_download(&saved);
        assert_eq!(ui.model_choice.selected(), 2);
        assert_eq!(ui.active_model.text(), active);
        assert!(ui.download.property::<bool>("visible"));
        ui.download.emit_clicked();
        assert_eq!(ui.model_choice.selected(), 0);
        wait_for_download(&saved);
        assert_eq!(ui.model_choice.selected(), 2);
        ui.model_choice.set_selected(3);
        ui.cancel_download.emit_clicked();
        wait_for_download(&saved);
        assert_eq!(ui.model_choice.selected(), 2);
        assert_eq!(ui.active_model.text(), active);

        // Qwen's directory and backend are applied together. Save errors also
        // restore the selection and retain the working model.
        ui.model_choice.set_selected(6);
        wait_for_download(&saved);
        assert!(
            manager
                .poll(&saved, true, |_| anyhow::bail!("Disk dolu"))
                .is_none()
        );
        assert_eq!(ui.model_choice.selected(), 2);
        ui.model_choice.set_selected(6);
        wait_for_download(&saved);
        saved = manager.poll(&saved, true, |_| Ok(())).unwrap();
        assert_eq!(saved.asr_backend, AsrBackend::Qwen3);
        assert_eq!(ui.model_choice.selected(), 6);
        assert!(ui.active_model.text().contains("Qwen3-ASR"));
        ui.navigation
            .select_row(ui.navigation.row_at_index(1).as_ref());
        ui.window.present();
        settle();
        capture(&ui, "qwen-selected");
        ui.window.destroy();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[ignore = "Requires a GTK display"]
    fn settings_navigation_search_and_model_selection() {
        gtk::init().unwrap();
        let provider = gtk::CssProvider::new();
        let errors = Rc::new(RefCell::new(Vec::new()));
        let parse_errors = errors.clone();
        provider.connect_parsing_error(move |_, _, error| {
            parse_errors.borrow_mut().push(error.to_string())
        });
        provider.load_from_string(CSS);
        assert!(
            errors.borrow().is_empty(),
            "CSS errors: {:?}",
            errors.borrow()
        );
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().unwrap(),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk::Settings::default()
            .unwrap()
            .set_gtk_application_prefer_dark_theme(false);
        let app = gtk::Application::builder()
            .application_id("io.github.justwhisper.UiTest")
            .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(None::<&gtk::gio::Cancellable>).unwrap();
        let initial = Config::default();
        let ui = settings_ui(&app, &initial, false);
        ui.window.present();
        settle();
        assert_eq!(ui.stack.visible_child_name().as_deref(), Some("dictation"));
        assert!(ui.pending.text().is_empty());
        capture(&ui, "dictation");

        // Edits survive navigation, and every page retains the fixed save action.
        ui.paste.set_selected(1);
        ui.noise_reduction.set_active(false);
        ui.retention.set_selected(3);
        ui.model_choice.set_selected(2);
        for (index, &(name, _, _, _)) in PAGES.iter().enumerate() {
            ui.navigation
                .select_row(ui.navigation.row_at_index(index as i32).as_ref());
            settle();
            assert_eq!(ui.stack.visible_child_name().as_deref(), Some(name));
            assert!(ui.save.is_mapped());
            assert!(ui.test.is_mapped());
            assert_eq!(ui.paste.selected(), 1);
            assert!(!ui.noise_reduction.is_active());
            assert_eq!(ui.model_choice.selected(), 2);
            assert_eq!(ui.retention.selected(), 3);
            assert_eq!(ui.pending.text().as_str(), "Kaydedilmemiş değişiklikler");
            capture(&ui, name);
        }

        // Turkish uppercase search, keywords, no results, and clearing the query.
        for (query, expected) in [
            ("GÜRÜLTÜ", "audio"),
            ("GPU", "performance"),
            ("İNDİR", "model"),
            ("YAPIŞTIRMA", "dictation"),
            ("no-such-setting", "empty"),
            ("", "dictation"),
        ] {
            ui.search.set_text(query);
            settle();
            assert_eq!(
                ui.stack.visible_child_name().as_deref(),
                Some(expected),
                "query: {query}"
            );
        }

        // Smaller windows must retain the sidebar and actions during a download.
        ui.window.destroy();
        let ui = settings_ui(&app, &initial, false);
        ui.window.set_default_size(900, 640);
        ui.navigation
            .select_row(ui.navigation.row_at_index(1).as_ref());
        ui.model_choice.set_selected(5);
        ui.download.set_label("İndiriliyor…");
        ui.cancel_download.set_visible(true);
        ui.progress.set_visible(true);
        ui.progress.set_fraction(0.42);
        ui.progress.set_text(Some("680 MB indirildi"));
        ui.window.present();
        settle();
        assert!(
            ui.window.width() <= 900,
            "window expanded to {}",
            ui.window.width()
        );
        for widget in [
            ui.save.upcast_ref::<gtk::Widget>(),
            ui.cancel_download.upcast_ref(),
            ui.test.upcast_ref(),
        ] {
            let bounds = widget.compute_bounds(&ui.window).unwrap();
            assert!(bounds.x() >= 0.0 && bounds.x() + bounds.width() <= ui.window.width() as f32);
            assert!(bounds.y() >= 0.0 && bounds.y() + bounds.height() <= ui.window.height() as f32);
        }
        capture(&ui, "model-compact-download");
        ui.window.destroy();

        let entry = gtk::Entry::builder()
            .text("/custom/models/ggml-small.bin")
            .build();
        let choice = model_selector(&entry);
        assert_eq!(choice.selected(), 2);
        assert_eq!(entry.text().as_str(), "/custom/models/ggml-small.bin");

        choice.set_selected(4);
        assert_eq!(
            std::path::Path::new(entry.text().as_str()),
            config::data_dir().join("models/ggml-large-v3.bin")
        );

        entry.set_text("/custom/models/ggml-base.bin");
        assert_eq!(choice.selected(), 4);
        assert_eq!(entry.text().as_str(), "/custom/models/ggml-base.bin");

        entry.set_text("/custom/models/ggml-large-v3-q5_0.bin");
        choice.set_selected(MODELS.len() as u32);
        assert_eq!(choice.selected(), MODELS.len() as u32);
        assert_eq!(
            entry.text().as_str(),
            "/custom/models/ggml-large-v3-q5_0.bin"
        );
        assert!(MODELS.get(choice.selected() as usize).is_none());

        let custom = gtk::Entry::builder().text("/custom/model.bin").build();
        let custom_choice = model_selector(&custom);
        assert_eq!(custom_choice.selected(), MODELS.len() as u32);
        assert_eq!(custom.text().as_str(), "/custom/model.bin");
        check_automatic_model_selection(&app);
    }
}
