use crate::{
    audio::{self, AudioBuffer, Capture},
    config::{AsrBackend, Config},
    noise::Processor,
    output,
    qwen::Qwen,
    residency::Resident,
    system_volume::SystemVolume,
};
use anyhow::{Context, Result};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub const IDLE: u8 = 0;
pub const RECORDING: u8 = 1;
pub const PROCESSING: u8 = 2;

pub enum Event {
    Status(String),
    Error(String),
    Recording(Arc<Mutex<AudioBuffer>>),
    Processing,
    Done(String),
    Shortcut(String),
    ShortcutConfiguration { portal_supported: bool },
    Backend(String),
}

enum Command {
    Start,
    Stop,
    Cancel,
    Configure(Config),
    Shutdown,
}

#[derive(Clone)]
pub struct Controller {
    sender: Sender<Command>,
    pub phase: Arc<AtomicU8>,
    pub abort: Arc<std::sync::atomic::AtomicBool>,
}

impl Controller {
    pub fn start(&self) {
        if self
            .phase
            .compare_exchange(IDLE, RECORDING, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let _ = self.sender.send(Command::Start);
        }
    }
    pub fn stop(&self) {
        if self
            .phase
            .compare_exchange(RECORDING, PROCESSING, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let _ = self.sender.send(Command::Stop);
        }
    }
    pub fn cancel(&self) {
        self.abort.store(true, Ordering::SeqCst);
        let _ = self.sender.send(Command::Cancel);
    }
    pub fn configure(&self, config: Config) {
        let _ = self.sender.send(Command::Configure(config));
    }
    pub fn shutdown(&self) {
        self.abort.store(true, Ordering::SeqCst);
        let _ = self.sender.send(Command::Shutdown);
    }
}

enum Model {
    Whisper(WhisperContext),
    Qwen(Qwen),
}

struct Loading {
    handle: Option<JoinHandle<Result<Model>>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl Loading {
    fn new(
        work: impl FnOnce(Arc<std::sync::atomic::AtomicBool>) -> Result<Model> + Send + 'static,
    ) -> Self {
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancel.clone();
        Self {
            handle: Some(thread::spawn(move || work(flag))),
            cancel,
        }
    }
    fn is_finished(&self) -> bool {
        self.handle
            .as_ref()
            .is_none_or(|handle| handle.is_finished())
    }
}
impl Drop for Loading {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

fn load(config: &Config, events: &Sender<Event>) -> Loading {
    let path = config.model_path.clone();
    let use_gpu = config.use_gpu;
    let asr_backend = config.asr_backend;
    let events = events.clone();
    Loading::new(move |cancel| {
        let _ = events.send(Event::Status("Model yükleniyor…".into()));
        if asr_backend == AsrBackend::Qwen3 {
            let model = Qwen::load(&path, use_gpu, &cancel)?;
            let _ = events.send(Event::Backend(model.backend.clone()));
            let _ = events.send(Event::Status("Qwen3-ASR hazır".into()));
            return Ok(Model::Qwen(model));
        }
        let mut params = WhisperContextParameters::default();
        let backend = selected_backend(use_gpu);
        params.use_gpu(backend.is_some());
        if let Some((index, _)) = &backend {
            params.gpu_device(*index);
        }
        let ctx = WhisperContext::new_with_params(
            path.to_str().context("Model yolu UTF-8 olmalı")?,
            params,
        )
        .context("Whisper modeli yüklenemedi")?;
        let _ = events.send(Event::Backend(
            backend
                .map(|(_, name)| format!("GPU · {name} · Vulkan"))
                .unwrap_or_else(|| "CPU".into()),
        ));
        let _ = events.send(Event::Status("Model hazır".into()));
        Ok(Model::Whisper(ctx))
    })
}

fn selected_backend(use_gpu: bool) -> Option<(i32, String)> {
    if !use_gpu {
        return None;
    }
    #[cfg(feature = "vulkan")]
    {
        // Use the same registry and GPU order as whisper_backend_init_gpu.
        // These generic bindings are available without a system libclang.
        unsafe {
            prefer_discrete(
                (0..whisper_rs_sys::ggml_backend_dev_count()).filter_map(|index| {
                    let device = whisper_rs_sys::ggml_backend_dev_get(index);
                    let kind = whisper_rs_sys::ggml_backend_dev_type(device);
                    if kind != whisper_rs_sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU
                        && kind
                            != whisper_rs_sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU
                    {
                        return None;
                    }
                    let name = std::ffi::CStr::from_ptr(
                        whisper_rs_sys::ggml_backend_dev_description(device),
                    )
                    .to_string_lossy();
                    Some((
                        kind == whisper_rs_sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU,
                        name.into_owned(),
                    ))
                }),
            )
        }
    }
    #[cfg(not(feature = "vulkan"))]
    {
        None
    }
}

#[cfg(any(feature = "vulkan", test))]
fn prefer_discrete(devices: impl IntoIterator<Item = (bool, String)>) -> Option<(i32, String)> {
    devices
        .into_iter()
        .enumerate()
        .min_by_key(|(_, (discrete, _))| !discrete)
        .map(|(ordinal, (_, name))| (ordinal as i32, name))
}

fn finish_load(mut loader: Loading) -> Result<Model> {
    loader
        .handle
        .take()
        .unwrap()
        .join()
        .map_err(|_| anyhow::anyhow!("Model yükleme iş parçacığı durdu"))?
}

unsafe extern "C" fn should_abort(data: *mut std::ffi::c_void) -> bool {
    // state.full is synchronous; transcribe retains the Arc for the entire call.
    let flag = unsafe { &*data.cast::<std::sync::atomic::AtomicBool>() };
    flag.load(Ordering::SeqCst)
}

fn transcribe(
    model: &mut Model,
    config: &Config,
    samples: Vec<f32>,
    abort: Arc<std::sync::atomic::AtomicBool>,
) -> Result<String> {
    match model {
        Model::Whisper(ctx) => transcribe_whisper(ctx, config, samples, abort),
        Model::Qwen(ctx) => ctx.transcribe(&samples, &config.language, abort),
    }
}

fn transcribe_whisper(
    ctx: &WhisperContext,
    config: &Config,
    mut samples: Vec<f32>,
    abort: Arc<std::sync::atomic::AtomicBool>,
) -> Result<String> {
    // whisper.cpp requires at least one second. Pad short phrases without dropping the beginning.
    samples.resize(samples.len().max(audio::SAMPLE_RATE), 0.0);
    let mut state = ctx.create_state()?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(
        thread::available_parallelism()
            .map(|n| n.get().min(8))
            .unwrap_or(4) as i32,
    );
    params.set_language(if config.language == "auto" {
        None
    } else {
        Some(&config.language)
    });
    params.set_translate(false);
    params.set_no_context(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    // whisper-rs 0.16's safe abort wrapper stores a trait object but casts it as
    // the concrete closure. Use its C callback API with a stable AtomicBool.
    unsafe {
        params.set_abort_callback(Some(should_abort));
        params.set_abort_callback_user_data(Arc::as_ptr(&abort).cast_mut().cast());
    }
    state.full(params, &samples)?;
    drop(abort);
    let mut text = String::new();
    for segment in state.as_iter() {
        text.push_str(segment.to_str()?);
    }
    Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub fn spawn(config: Config, events: Sender<Event>) -> Controller {
    spawn_with(config, events, Capture::start, load, output::insert)
}

fn spawn_with(
    mut config: Config,
    events: Sender<Event>,
    capture_audio: impl Fn() -> Result<Capture> + Send + 'static,
    load_model: impl Fn(&Config, &Sender<Event>) -> Loading + Send + 'static,
    insert_text: impl Fn(&str, &str) -> Result<()> + Send + 'static,
) -> Controller {
    let (sender, receiver) = mpsc::channel();
    let phase = Arc::new(AtomicU8::new(IDLE));
    let abort = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let controller = Controller {
        sender,
        phase: phase.clone(),
        abort: abort.clone(),
    };
    thread::spawn(move || {
        let mut capture: Option<Capture> = None;
        let mut system_volume: Option<SystemVolume> = None;
        let mut resident = Resident::<Model>::default();
        let mut loading: Option<Loading> = None;
        let mut noise = Processor::default();
        if !config.load_on_press && config.validate().is_ok() {
            loading = Some(load_model(&config, &events));
        }
        loop {
            // Reap eager loading so failures appear even without pressing the shortcut.
            if capture.is_none() && loading.as_ref().is_some_and(|h| h.is_finished()) {
                match finish_load(loading.take().unwrap()) {
                    Ok(ctx) => {
                        resident.insert(ctx, Instant::now());
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("{e:#}")));
                    }
                }
            }
            if resident.expire(
                &config,
                Instant::now(),
                capture.is_some() || phase.load(Ordering::SeqCst) != IDLE,
            ) {
                let _ = events.send(Event::Backend("Model bellekten çıkarıldı".into()));
            }
            let command = match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(c) => c,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            match command {
                Command::Shutdown => {
                    drop(capture.take());
                    drop(system_volume.take());
                    break;
                }
                Command::Configure(next) => {
                    let changed = next.model_path != config.model_path
                        || next.use_gpu != config.use_gpu
                        || next.asr_backend != config.asr_backend;
                    if changed {
                        resident.clear();
                        // Dropping a JoinHandle detaches loading. Do not delay a new
                        // microphone capture while an obsolete model finishes I/O.
                        loading = None;
                    }
                    if !config.load_on_press && next.load_on_press {
                        resident.finish_use(Instant::now());
                    }
                    config = next;
                    if !config.noise_reduction {
                        noise.release();
                    }
                    if !config.load_on_press
                        && resident.model.is_none()
                        && loading.is_none()
                        && config.validate().is_ok()
                    {
                        loading = Some(load_model(&config, &events));
                    }
                }
                Command::Start => {
                    abort.store(false, Ordering::SeqCst);
                    let result = config.validate().and_then(|_| capture_audio());
                    match result {
                        Ok(recording) => {
                            // Start capture BEFORE launching model I/O. Every sample is retained.
                            let _ = events.send(Event::Recording(recording.buffer.clone()));
                            capture = Some(recording);
                            if resident.model.is_none() && loading.is_none() {
                                loading = Some(load_model(&config, &events));
                            }
                            match SystemVolume::duck(config.system_volume_percent) {
                                Ok(volume) => system_volume = volume,
                                Err(error) => {
                                    let _ = events.send(Event::Status(format!(
                                        "Sistem sesi ayarlanamadı: {error:#}"
                                    )));
                                }
                            }
                        }
                        Err(e) => {
                            phase.store(IDLE, Ordering::SeqCst);
                            let _ = events.send(Event::Error(format!("{e:#}")));
                        }
                    }
                }
                Command::Cancel => {
                    drop(capture.take());
                    drop(system_volume.take());
                    if config.load_on_press {
                        loading = None;
                    }
                    resident.finish_use(Instant::now());
                    phase.store(IDLE, Ordering::SeqCst);
                    let _ = events.send(Event::Done("Kayıt iptal edildi".into()));
                }
                Command::Stop => {
                    drop(system_volume.take());
                    let Some(recording) = capture.take() else {
                        continue;
                    };
                    let _ = events.send(Event::Processing);
                    let result: Result<String> = (|| {
                        let samples = recording.finish()?;
                        if config.noise_reduction
                            && audio::has_speech(&samples, audio::CAPTURE_RATE)
                        {
                            let _ = events.send(Event::Status("Gürültü azaltılıyor…".into()));
                        }
                        let samples = noise.prepare(samples, config.noise_reduction, &abort)?;
                        if !audio::has_speech(&samples, audio::SAMPLE_RATE) {
                            return Ok(String::new());
                        }
                        if resident.model.is_none() {
                            let loader = loading.take().context("Model yükleme başlamadı")?;
                            while !loader.is_finished() {
                                if abort.load(Ordering::SeqCst) {
                                    return Ok(String::new());
                                }
                                thread::sleep(Duration::from_millis(50));
                            }
                            resident.insert(finish_load(loader)?, Instant::now());
                        }
                        if abort.load(Ordering::SeqCst) {
                            return Ok(String::new());
                        }
                        transcribe(
                            resident.model.as_mut().unwrap(),
                            &config,
                            samples,
                            abort.clone(),
                        )
                    })();
                    let outcome = match result {
                        Ok(text) if text.is_empty() => Event::Done("Konuşma algılanmadı".into()),
                        Ok(text) if !abort.load(Ordering::SeqCst) => {
                            let _ = events.send(Event::Status("Aktif alana yazılıyor…".into()));
                            match insert_text(&text, &config.paste_shortcut) {
                                Ok(()) => Event::Done(text),
                                Err(e) => Event::Error(format!("{e:#}")),
                            }
                        }
                        Ok(_) => Event::Done("İptal edildi".into()),
                        Err(e) if !abort.load(Ordering::SeqCst) => Event::Error(format!("{e:#}")),
                        Err(_) => Event::Done("İptal edildi".into()),
                    };
                    if config.asr_backend == AsrBackend::Qwen3
                        && (abort.load(Ordering::SeqCst) || matches!(outcome, Event::Error(_)))
                    {
                        resident.clear();
                    }
                    resident.finish_use(Instant::now());
                    // Silent takes leave loading to be reaped by the idle loop.
                    phase.store(IDLE, Ordering::SeqCst);
                    let _ = events.send(outcome);
                }
            }
        }
    });
    controller
}

pub fn transcribe_file(config: &Config, audio: &std::path::Path) -> Result<String> {
    config.validate()?;
    let output = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(audio)
        .args([
            "-t", "120", "-ac", "1", "-ar", "48000", "-f", "f32le", "pipe:1",
        ])
        .output()
        .context("Ses dosyası açılamadı")?;
    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    let samples: Vec<_> = output
        .stdout
        .chunks_exact(4)
        .map(|bytes| {
            let sample = f32::from_le_bytes(bytes.try_into().unwrap());
            if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        })
        .collect();
    let abort = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let samples = Processor::default().prepare(samples, config.noise_reduction, &abort)?;
    if !audio::has_speech(&samples, audio::SAMPLE_RATE) {
        return Ok(String::new());
    }
    let (events, _) = mpsc::channel();
    let mut model = finish_load(load(config, &events))?;
    transcribe(&mut model, config, samples, abort)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Requires the installed Qwen model/runtime and QWEN_TEST_AUDIO pointing to jfk.wav"]
    fn qwen_reuses_model_during_recording_and_reloads_after_idle_expiry() {
        let audio = std::env::var("QWEN_TEST_AUDIO").unwrap();
        let config = Config {
            asr_backend: AsrBackend::Qwen3,
            model_path: crate::qwen::model_path(),
            language: "en".into(),
            load_on_press: true,
            model_keep_alive_secs: 1,
            noise_reduction: false,
            ..Config::default()
        };
        let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = loads.clone();
        let (events, receiver) = mpsc::channel();
        let controller = spawn_with(
            config,
            events,
            move || {
                let mut command = std::process::Command::new("ffmpeg");
                command
                    .args([
                        "-nostdin",
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-re",
                        "-stream_loop",
                        "-1",
                        "-i",
                    ])
                    .arg(&audio)
                    .args([
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
                Capture::from_command(command)
            },
            move |config, events| {
                count.fetch_add(1, Ordering::SeqCst);
                load(config, events)
            },
            |_, _| Ok(()),
        );

        let take = || {
            controller.start();
            let buffer = loop {
                match receiver.recv_timeout(Duration::from_secs(30)).unwrap() {
                    Event::Recording(buffer) => break buffer,
                    Event::Error(error) => panic!("{error}"),
                    _ => {}
                }
            };
            let deadline = Instant::now() + Duration::from_secs(10);
            while buffer.lock().unwrap().samples.len() < 3 * audio::CAPTURE_RATE {
                assert!(Instant::now() < deadline, "Capture timed out");
                thread::sleep(Duration::from_millis(10));
            }
            controller.stop();
            loop {
                match receiver.recv_timeout(Duration::from_secs(30)).unwrap() {
                    Event::Done(text) => {
                        assert!(text.to_lowercase().contains("americ"), "{text}");
                        break;
                    }
                    Event::Backend(text) if text == "Model bellekten çıkarıldı" => {
                        panic!("Model expired during a recording")
                    }
                    Event::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
        };
        take();
        take();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            1,
            "Second recording must reuse the model"
        );
        loop {
            match receiver.recv_timeout(Duration::from_secs(10)).unwrap() {
                Event::Backend(text) if text == "Model bellekten çıkarıldı" => break,
                Event::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
        take();
        assert_eq!(loads.load(Ordering::SeqCst), 2, "Expired model must reload");
        controller.shutdown();
    }

    #[test]
    fn prefers_discrete_gpu_even_when_integrated_gpu_is_listed_first() {
        let selected = prefer_discrete([(false, "Intel".into()), (true, "RTX 4060".into())]);
        assert_eq!(selected, Some((1, "RTX 4060".into())));
        assert_eq!(
            prefer_discrete([(false, "Intel".into())]),
            Some((0, "Intel".into()))
        );
        assert_eq!(prefer_discrete([]), None);
    }
    fn reference_config() -> Config {
        Config {
            model_path: std::env::var("WHISPER_TEST_MODEL").unwrap().into(),
            language: "en".into(),
            ..Config::default()
        }
    }
    #[test]
    #[ignore = "Requires WHISPER_TEST_MODEL and WHISPER_TEST_AUDIO pointing to jfk.wav"]
    fn transcribes_official_reference_audio() {
        let config = reference_config();
        let audio = std::env::var("WHISPER_TEST_AUDIO").unwrap();
        let result = transcribe_file(&config, std::path::Path::new(&audio))
            .unwrap()
            .to_lowercase();
        assert!(
            result.contains("ask not what your country can do for you"),
            "{result}"
        );
        assert!(
            result.contains("ask what you can do for your country"),
            "{result}"
        );
    }

    #[test]
    #[ignore = "Requires WHISPER_TEST_MODEL and WHISPER_TEST_AUDIO pointing to jfk.wav"]
    fn transcribes_reference_speech_mixed_with_background_noise() {
        let config = reference_config();
        let audio = std::env::var("WHISPER_TEST_AUDIO").unwrap();
        let output = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(audio)
            .args([
                "-f",
                "lavfi",
                "-i",
                "anoisesrc=color=pink:amplitude=0.1:sample_rate=48000:seed=17",
                "-filter_complex",
                "[0:a][1:a]amix=inputs=2:duration=first:normalize=0",
                "-ac",
                "1",
                "-ar",
                "48000",
                "-f",
                "f32le",
                "pipe:1",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let samples = output
            .stdout
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
            .collect();
        let abort = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let samples = Processor::default().prepare(samples, true, &abort).unwrap();
        let (events, _) = mpsc::channel();
        let mut model = finish_load(load(&config, &events)).unwrap();
        let text = transcribe(&mut model, &config, samples, abort)
            .unwrap()
            .to_lowercase();
        // The recognizer may omit the plural ending under noise; require the
        // opening words as well as both complete phrases below.
        assert!(text.contains("and so my fellow american"), "{text}");
        assert!(
            text.contains("ask not what your country can do for you"),
            "{text}"
        );
        assert!(
            text.contains("ask what you can do for your country"),
            "{text}"
        );
    }

    #[test]
    #[ignore = "Requires WHISPER_TEST_MODEL and WHISPER_TEST_AUDIO pointing to jfk.wav"]
    fn release_before_model_loaded_preserves_first_words() {
        let audio = std::env::var("WHISPER_TEST_AUDIO").unwrap();
        let gate = Arc::new(std::sync::Barrier::new(2));
        let model_gate = gate.clone();
        let loaded = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let model_loaded = loaded.clone();
        let (events, receiver) = mpsc::channel();
        let controller = spawn_with(
            reference_config(),
            events,
            move || {
                let mut command = std::process::Command::new("ffmpeg");
                command
                    .args([
                        "-nostdin",
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-re",
                        "-stream_loop",
                        "-1",
                        "-i",
                    ])
                    .arg(&audio)
                    .args([
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
                Capture::from_command(command)
            },
            move |config, events| {
                let config = config.clone();
                let events = events.clone();
                let gate = model_gate.clone();
                let loaded = model_loaded.clone();
                Loading::new(move |_| {
                    gate.wait();
                    let model = finish_load(load(&config, &events));
                    loaded.store(true, Ordering::SeqCst);
                    model
                })
            },
            |_, _| Ok(()),
        );
        controller.start();
        let buffer = loop {
            match receiver.recv_timeout(Duration::from_secs(5)).unwrap() {
                Event::Recording(buffer) => break buffer,
                Event::Error(e) => panic!("{e}"),
                _ => {}
            }
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while buffer.lock().unwrap().samples.len() < 11 * audio::CAPTURE_RATE {
            assert!(
                std::time::Instant::now() < deadline,
                "Audio capture timed out"
            );
            thread::sleep(Duration::from_millis(20));
        }
        controller.stop();
        loop {
            match receiver.recv_timeout(Duration::from_secs(5)).unwrap() {
                Event::Processing => break,
                Event::Error(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert!(
            !loaded.load(Ordering::SeqCst),
            "Model must still be loading at release"
        );
        assert_eq!(controller.phase.load(Ordering::SeqCst), PROCESSING);
        gate.wait();
        let result = loop {
            // An unoptimized DFN3/Whisper test build can take longer than the
            // previous Whisper-only pipeline, especially during compilation.
            match receiver.recv_timeout(Duration::from_secs(60)).unwrap() {
                Event::Done(text) => break text.to_lowercase(),
                Event::Error(e) => panic!("{e}"),
                _ => {}
            }
        };
        controller.shutdown();
        assert!(
            result.contains("and so my fellow americans"),
            "Initial audio lost: {result}"
        );
        assert!(
            result.contains("ask what you can do for your country"),
            "{result}"
        );
    }
}
