# Just Whisper

Rust + GTK4 ile GNOME Wayland üzerinde basılı tutarak konuşmayı metne çevirir. Whisper modellerini `whisper-rs` / `whisper.cpp`, Qwen3-ASR 1.7B modelini ayrı bir yerel Python süreciyle çalıştırır. Ses bilgisayardan çıkmaz, API anahtarı gerekmez.

Global kısayola basınca mikrofon kaydı başlar. Siyah pill üzerinde beyaz frekans çubukları sesi gösterir. Tuşu bırakınca pill yuvarlağa daralır ve dönen işleme animasyonu görünür. Sonuç panoya konur, `ydotool` aktif alana yapıştırır. Bu yöntem Türkçe karakterleri klavye düzeninden bağımsız aktarır. Panonun mevcut içeriği sonuçla değiştirilir.

## Gereksinimler

- Rust 1.92+, C/C++ derleyici, CMake ve pkg-config.
- GTK4 4.12+ geliştirme dosyaları.
- Vulkan geliştirme dosyaları, `glslc` ve ekran kartının Vulkan sürücüsü.
- `ffmpeg`, PulseAudio/PipeWire Pulse sunucusu, `curl`, `wl-copy`, `ydotool` ve çalışan `ydotoold`.
- GNOME 48–50. Masaüstü kısayolu için `xdg-desktop-portal-gnome`; sağ AltGr için `/dev/input` okuma erişimi.
- GNOME overlay için birlikte gelen Shell eklentisi.

Fedora:

```sh
sudo dnf install gtk4-devel gcc-c++ cmake pkgconf-pkg-config vulkan-loader-devel glslc ffmpeg-free curl wl-clipboard ydotool
```

Ubuntu/Debian:

```sh
sudo apt install libgtk-4-dev build-essential cmake pkg-config libvulkan-dev glslc ffmpeg curl wl-clipboard ydotool
```

whisper-rs hazır bağlayıcı dosyaları kullandığı için libclang gerekmez. Varsayılan derleme Vulkan GPU desteği içerir. "GPU hızlandırma" açıkken model ekran kartında çalışır; uygulama hibrit makinelerde ayrık ekran kartını seçer. GPU bulunmazsa CPU kullanılır. Kullanılan aygıt model yüklenince arayüzde gösterilir. GPU ayarını değiştirmek bellekteki modeli yeniden yükler.

CUDA Toolkit gerekmez. GPU kullanmak istemiyorsanız arayüzde "GPU hızlandırma" seçeneğini kapatın. Vulkan geliştirme araçları olmayan bir makinede yalnızca CPU için `cargo build --release --no-default-features` ile derlenebilir.

## Çalıştırma

```sh
bash scripts/download-model.sh base
bash scripts/install.sh
~/.local/bin/just-whisper
```

Ayarlar sol menüde Dikte, Konuşma modeli, Ses, Performans, Uygulama ve Hakkında sayfalarına ayrılır. Arama alanı ayar adlarıyla ilgili sayfayı bulur. Sayfalar arasında geçiş, kaydedilmemiş değişiklikleri korur. Alttaki "Ayarları kaydet" düğmesi davranış, dil ve performans ayarlarını birlikte uygular. Model seçimi otomatik uygulanır. Kayıt durumu ve test düğmesi sol menünün altında kalır.

Arayüzde Whisper Tiny, Base, Small, Medium, Large v3, Large v3 Turbo ve Qwen3-ASR 1.7B seçilebilir. İndirme ilerlemesi gösterilir; "Durdur" ile durdurulan indirme, aynı modeli tekrar seçince kaldığı yerden devam eder. Varsayılan çok dilli `base`, varsayılan dil Türkçedir. Large v3 yaklaşık 3,1 GB, Turbo yaklaşık 1,6 GB indirme gerektirir. Nicemlenmiş bir whisper.cpp GGML modelini dosya seçiciyle de açabilirsiniz. `.en` modelleri Türkçe için uygun değildir.

Model listesi açılışta kayıtlı modeli gösterir. Listeden seçtiğiniz model eksikse otomatik indirilir; hazır olunca etkinleşir ve seçim kaydedilir. Model için dosya yolu seçmeye veya "Ayarları kaydet" düğmesine basmaya gerek yoktur. Kayıt/işleme sürüyorsa geçiş bittikten sonra yapılır. İndirme başarısız olur veya durdurulursa önceki model seçili ve etkin kalır; "Tekrar dene" başarısız indirmeyi sürdürür. "Etkin model" satırı kullanılan modeli gösterir. Özel GGML dosyaları listede "Özel Whisper dosyası…" seçilerek açılır; dosya seçimi otomatik uygulanır, elle girilen yolu Enter ile uygulayabilirsiniz.

İlk kurulum masaüstü girişini oluşturur; portal bu uygulama kimliğini gerektirir. Geliştirme sırasında bundan sonra `cargo run` kullanılabilir.

İlk açılışta masaüstü global kısayol izni ister. Tercih edilen kısayol `Super + Shift + Space`; portalın onayladığı gerçek kısayol arayüzde gösterilir. GNOME Ayarlar → Klavye → Kısayolları görüntüle ve özelleştir → Uygulama kısayolları bölümünden değiştirilebilir. Portal v2 destekleyen masaüstlerinde "Kısayolu değiştir" düğmesi yapılandırma penceresini açar. Portal v1 kullanan GNOME'da düğme "GNOME ayarlarını aç" olarak görünür ve klavye ayarlarını açar; mevcut kısayol çalışmaya devam eder.

"Konuşma tuşu" alanından "Sağ AltGr" seçip ayarları kaydederseniz uygulama GNOME kısayol portalı yerine Linux `evdev` olaylarını okur. Sağ AltGr tek başına basılı tutulduğunda kayıt başlar, bırakıldığında metne çevirme başlar. Sol Alt ve tuş tekrarı tetiklemez. AltGr ile başka bir tuşa basmak, örneğin `AltGr + Q`, kaydı iptal eder ve normal karakter yazımını korur. Klavye tekelleştirilmez; tuşlar diğer uygulamalara ulaşmaya devam eder. Bu yöntem [Vocalinux'un evdev yaklaşımını](https://github.com/VocaHQ/vocalinux/blob/main/src/vocalinux/ui/keyboard_backends/evdev_backend.py) kullanır; kod Rust'ta yazılmıştır.

AltGr modunda klavye olay dosyaları yalnızca okuma amacıyla açılır. Erişim eksikse arayüz bunu bildirir; uygulama otomatik olarak sistem izinlerini değiştirmez. Gerekirse:

```sh
sudo usermod -aG input "$USER"
```

Ardından oturumu kapatıp açın. `input` grubu klavye aygıtlarına genel okuma erişimi verir. Uygulama tuşlarla yazılan metni kaydetmez; yalnızca basılı tuş durumlarını konuşma tuşu için takip eder. GNOME ekran kilitliyken kayıt başlatılmaz ve mevcut kayıt/işlem iptal edilir. Kilit durumu okunamazsa AltGr devre dışı kalır. Klavye çıkarıldığında basılı tuş durumu temizlenir; yeni takılan klavyeler otomatik algılanır. Masaüstü kısayoluna geri dönmek için aynı alandan "Masaüstü kısayolu" seçip kaydedin.

## GNOME kurulumu

```sh
bash scripts/install.sh
```

Kurulum derlenmiş uygulamayı `~/.local/bin/just-whisper`, masaüstü girişini uygulamalar dizinine ve eklentiyi `~/.local/share/gnome-shell/extensions/just-whisper@local` altına kopyalar. Eklentiyi mevcut etkin eklentileri koruyarak açar. GNOME yeni eklentiyi oturum sırasında görmezse bir sonraki oturum için etkinleştirir; oturumu kapatıp açın. Eklentiyi sonradan elle açmak için:

```sh
gnome-extensions enable just-whisper@local
```

Uygulama her açılışta `systemctl --user start ydotoold.service` ile yapıştırma servisini başlatır. Sistem açılışında arka planda başlatıldığında da bu işlem yapılır. Servis zaten çalışıyorsa yeniden başlatılmaz. İşlem ayrı bir iş parçacığında yürür; arayüz açılmasını bekletmez. Başlatma hatası arayüzde gösterilir.

`ydotoold` kullanıcı servisi kurulu olmalı ve kullanıcının `/dev/uinput` erişimi bulunmalıdır. Dağıtımın ydotool servisi farklıysa kendi servis ayarını kullanın. `YDOTOOL_SOCKET` ortam değişkeni özel soket yolunu destekler. GNOME üzerinde `wtype` kullanılmaz.

Panoya kopyalandıktan sonra uygulama yapıştırmadan önce 150 ms bekler. Bu süre, `wl-copy` geçici bir pencereyle odağı aldığında odağın metin alanına dönmesine ve pano güncellemesinin ulaşmasına zaman verir. Yapıştırma tuşları 25 ms aralıklarla gönderilir.

Pencereyi kapatmak uygulamayı arka planda bırakır. Tamamen kapatmak için "Uygulamadan çık" düğmesini kullanın. Yalnızca arka planda başlatmak için:

```sh
just-whisper --background
```

Uygulama sayfasındaki "Sistem açılışında başlat" seçeneğini açıp "Ayarları kaydet" düğmesine basın. Uygulama sonraki oturum açılışlarında pencere göstermeden arka planda başlar. Seçenek varsayılan olarak kapalıdır. Kapatıp kaydetmek otomatik başlatmayı kaldırır. Başlangıç kaydı `$XDG_CONFIG_HOME/autostart/io.github.justwhisper.App.desktop` dosyasındadır; XDG değişkeni yoksa `~/.config/autostart` kullanılır. Kayıt, ayarı kaydeden uygulamanın tam dosya yolunu kullanır. Kalıcı kullanım için kurulu uygulamadan etkinleştirin.

## Qwen3-ASR 1.7B

Konuşma modeli sayfasından **Qwen3-ASR 1.7B** seçin. İlk seçim, `uv` ile ayrı bir Python 3.12 ortamı kurar, `qwen-asr==0.0.6` ve `torch==2.10.0` paketlerini yükler, ardından resmi `Qwen/Qwen3-ASR-1.7B` modelini indirir. `uv` PATH üzerinde veya `~/.local/bin/uv` konumunda bulunmalıdır. Eksikse arayüz kurulum bağlantısını gösterir: [uv kurulumu](https://docs.astral.sh/uv/getting-started/installation/). Sistem Python paketleri değiştirilmez. Model yaklaşık 4,7 GB, çalışma ortamı da birkaç GB disk alanı kullanır.

Qwen çalışma ortamı `$XDG_DATA_HOME/just-whisper/qwen-runtime/`, model `$XDG_DATA_HOME/just-whisper/models/qwen3-asr-1.7b/` altında tutulur. Çalıştırıcı ve kurulum betiği Rust uygulamasına gömülüdür; kurulu uygulama kaynak dizinine ihtiyaç duymaz. Model sürümü sabittir, indirme tamamlanınca hazır işareti yazılır. Tuşa basmak sırasında indirme yapılmaz; tanıma yalnızca yerel dosyalarla ve çevrimdışı modda çalışır.

"GPU hızlandırma" açıkken Qwen PyTorch CUDA kullanır; CUDA aygıtı yoksa CPU'ya geçer. NVIDIA sürücüsü gerekir, ayrıca CUDA Toolkit kurmak gerekmez. Qwen Vulkan kullanmaz. CPU çalıştırma da desteklenir, ancak özellikle 1.7B modelinde daha yavaştır. Whisper'ın mevcut Vulkan desteği devam eder.

Türkçe, İngilizce ve otomatik dil seçimi desteklenir. Mikrofon, DeepFilterNet3 gürültü azaltma, iki dakikalık kayıt sınırı, overlay ve yapıştırma akışı iki model için ortaktır. Ses, 16 kHz mono PCM olarak doğrudan süreçler arasındaki borudan aktarılır; ham kayıt diske veya ağa gönderilmez. İptal, Qwen yüklemesini ve tanımayı durdurur. "Modeli tuşa basınca yükle" kapalıysa Python süreci ve model sonraki kayıtlar için bellekte tutulur; açıksa son kullanımdan sonra seçilen süre boyunca hazır tutulur ve süre dolunca kapatılır.

Modeli pencere açmadan denemek için:

```sh
just-whisper --transcribe recording.wav --backend qwen3 --language tr
```

Kurulumu arayüzden bağımsız hazırlamak için:

```sh
bash scripts/setup-qwen.sh
~/.local/share/just-whisper/qwen-runtime/bin/python scripts/qwen-worker.py download \
  ~/.local/share/just-whisper/models/qwen3-asr-1.7b
```

Çalıştırma API'si [Qwen'in resmi Transformers örneğine](https://github.com/QwenLM/Qwen3-ASR#python-package-usage) dayanır.

## Model yükleme ayarı

"Modeli tuşa basınca yükle" açıkken mikrofon kayıt süreci model yüklemeden önce başlatılır. Kayıt 48 kHz mono PCM olarak bellekte birikir; seçilen model ayrı iş parçacığında yüklenir. Tuş model yüklenmeden bırakılırsa kayıt durur, gürültü azaltma ve 16 kHz dönüşümü tamamlanır, model hazır olunca ilk örnek dahil bütün kayıt modele verilir. Metin çıktıktan sonra model, "Modeli bellekte tutma süresi" kadar hazır kalır. Varsayılan süre 5 dakikadır. Yeni bir kayıt modeli tekrar kullanır ve kayıt/işleme bittikten sonra süre baştan başlar. Model kayıt veya işleme sırasında boşaltılmaz.

Performans sayfasındaki "Modeli bellekte tutma süresi" alanından Hemen boşalt, 1, 5, 10 veya 30 dakika seçebilirsiniz. GPU açıkken model VRAM, CPU ile çalışırken RAM kullanır. Süre dolunca Whisper bağlamı veya Qwen Python süreci kapatılır ve model belleği serbest bırakılır. Sonraki kayıt sırasında yeniden yüklenir. Model veya GPU aygıtı değişince eski model beklemeden boşaltılır. Ayar dosyasında `model_keep_alive_secs` saniye cinsindendir; eski ayar dosyaları da 300 saniyeyi kullanır.

"Modeli tuşa basınca yükle" kapalıyken uygulama modeli önceden yükler ve sonraki kayıtlar için sürekli bellekte tutar; süre seçimi bu modda devre dışıdır. Böylece model yükleme beklemesi ortadan kalkar, bellek kullanımı artar. Ayar değişiklikleri "Ayarları kaydet" düğmesiyle uygulanır.

Mikrofon yalnızca kayıt sırasında açılır. İki dakikalık üst sınırda kayıt otomatik durur. Mikrofon hatasında veya boş kayıtta sonuç yazılmaz. Çeviri sırasında yeni kayıt kabul edilmez. "İptal" kaydı/işlemeyi durdurur. Kısayol kaybolursa kayıt iptal edilir.

## Gürültü azaltma

"Gürültü azaltma" varsayılan olarak açıktır; eski ayar dosyaları da bu varsayılanı kullanır. DeepFilterNet3, tuş bırakıldıktan sonra 48 kHz kayıttaki arka plan gürültüsünü CPU üzerinde azaltır. Ardından ses, frekans katlanmasını önleyen yeniden örnekleme ile tanıma için 16 kHz'e dönüştürülür. İşlem sırasında "Gürültü azaltılıyor…" görünür. "İptal" temizlemeyi de durdurur.

Gürültü azaltma modeli uygulama dosyasına gömülüdür; bu filtre için ek model indirme, Python veya ağ bağlantısı gerekmez. İlk sesli kayıtta hazırlanır ve sonraki kayıtlar için bellekte tutulur. Her kayıt ayrı filtre durumuyla başlar. Son ses paketleri sıfırla doldurulur, filtre gecikmesi telafi edilir ve kaydın uzunluğu korunur. Sessizlik ve çok kısa tuş basmaları filtreyi yüklemez. Temizleme sonrası enerji kontrolü, yeterince bastırılmış gürültünün Whisper'a gitmesini engeller; bu kontrol tam bir konuşma algılayıcısı değildir.

Kafe, fan ve benzeri arka plan seslerini azaltmak için kullanılabilir. Yakındaki kişilerin konuşmalarını tamamen ayırma garantisi yoktur. Mikrofonu ağza yakın tutmak yardımcı olur. Sessiz ortamda sesiniz bozulursa seçeneği kapatıp "Ayarları kaydet" düğmesine basın. Bu, filtre modelini bellekten çıkarır. Filtre hatası arayüzde gösterilir; aynı seçenekle temizlemeden devam edebilirsiniz.

Uygulama [DeepFilterNet'in yerel Rust/Tract uygulamasını](https://github.com/Rikorose/DeepFilterNet/tree/d375b2d8309e0935d165700c91da9de862a99c31/libDF) ve bu sürümün DeepFilterNet3 modelini kullanır. Kaynak sürümü ve uyumlu Tract bağımlılıkları sabittir; yeniden derleme aynı API ve modeli kullanır. Tract 0.21.4'ün geliştirme derlemesindeki grafik adı kontrolü gömülü modelin ürettiği yinelenen bias adlarını reddettiği için yalnızca `tract-core` bağımlılığının debug kontrolleri kapalıdır. Uygulamanın debug kontrolleri açıktır. MIT lisans bildirimi kurulumla `just-whisper/licenses/` dizinine kopyalanır.

## Tuş basılıyken sistem sesi

Ayarlar > Ses bölümündeki slider, konuşma tuşu basılıyken sistem çıkış sesini mevcut düzeyin seçilen oranına indirir. %100 ses düzeyini değiştirmez, %0 sesi kapatır. Tuş bırakılınca veya kayıt iptal edilince önceki ses düzeyi ve sessiz durumu geri yüklenir. PipeWire için `wpctl`, yoksa PulseAudio uyumlu `pactl` kullanılır.

## Overlay

GNOME, GTK layer-shell protokolünü desteklemediği için Shell eklentisi göstergeyi çizer. GTK4 uygulaması oturum D-Bus hattında `io.github.justwhisper.App` adından `/io/github/justwhisper/App` yolundaki `io.github.justwhisper.Overlay.Update(sad)` sinyalini gönderir. Durum `hidden`, `recording` veya `processing`; dizi gerçek sesin FFT bantlarıdır. Eklenti odağı ve klavye girişini almaz, aktif pencerenin monitöründe altta görünür. Uygulama kapanırsa veya sinyal kesilirse gizlenir.

Bu sürüm GNOME Wayland içindir. Diğer Wayland masaüstlerinde portal desteğine bağlı olarak kısayol ve ydotool çalışabilir; overlay için o masaüstüne uygun bir uygulama gerekir.

## Dosyalar ve doğrulama

Ayarlar `$XDG_CONFIG_HOME/just-whisper/config.json`, modeller `$XDG_DATA_HOME/just-whisper/models/` altında tutulur. XDG değişkenleri yoksa `~/.config` ve `~/.local/share` kullanılır. Ham kayıt diske kaydedilmez.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

GTK oturumunda sayfa geçişlerini, Türkçe aramayı, model seçimini ve küçük pencerede indirme kontrollerini doğrulamak için:

```sh
cargo test ui::tests::settings_navigation_search_and_model_selection -- --ignored --test-threads=1
```

Kurulu Qwen modeliyle bellek süresini doğrulamak için referans `jfk.wav` dosyasını kullanın:

```sh
QWEN_TEST_AUDIO=/tmp/just-whisper-jfk.wav cargo test qwen_reuses_model_during_recording_and_reloads_after_idle_expiry -- --ignored --test-threads=1
```

Bu test bir saniyelik bekleme süresiyle art arda iki kaydın tek model yüklemesini kullanmasını, kayıt sürerken modelin boşaltılmamasını ve süre dolduktan sonra sonraki kaydın modeli yeniden yüklemesini doğrular. Mikrofon açmaz veya metin yapıştırmaz.

Gerçek model ve model yüklenmeden tuş bırakma testi için whisper.cpp'nin `samples/jfk.wav` referansını indirin:

```sh
curl -fL https://raw.githubusercontent.com/ggml-org/whisper.cpp/master/samples/jfk.wav -o /tmp/just-whisper-jfk.wav
WHISPER_TEST_MODEL="$HOME/.local/share/just-whisper/models/ggml-base.bin" \
WHISPER_TEST_AUDIO=/tmp/just-whisper-jfk.wav cargo test engine::tests:: -- --ignored --test-threads=1
```

Bu testler temiz referans konuşmayı, aynı konuşmaya eklenmiş pembe gürültüyü ve model hazır olmadan tuş bırakma senaryosunu DeepFilterNet3 açıkken doğrular. Son test mikrofon yerine referans dosyasını gerçek zamanlı okur, modelin hazır olmasını özellikle bekletir, tuş bırakıldıktan sonra modeli açar ve başlangıç kelimelerinin çıktıda kaldığını kontrol eder. Gerçek mikrofon veya aktif alana yazma testine gerek kalmadan kayıt/model eşzamanlılığı doğrulanır. Standart testler ayrıca gerçek DFN3 modeliyle gürültü enerjisinin düşmesini ve art arda kayıtların birbirini etkilememesini kontrol eder.

Gerçek kernel giriş olaylarını doğrulamak için çalışan `ydotoold` ve klavye okuma erişimiyle aşağıdaki testi çalıştırabilirsiniz. Sanal klavyeden bir AltGr basma/bırakma olayı üretir; ses kaydetmez veya metin yazmaz:

```sh
cargo test kernel_events_preserve_right_alt_press_and_release -- --ignored
```

Gerçek Wayland oturumunda odaklı GTK alanına 20 ardışık Türkçe ve Unicode yapıştırmayı doğrulamak için aşağıdaki testi çalıştırabilirsiniz. Test ayrı bir pencere açar, panoyu değiştirir ve bitince temizler:

```sh
cargo test repeatedly_pastes_unicode_into_focused_entry -- --ignored
```

Ses dosyasını pencere açmadan çevirmek için:

```sh
just-whisper --transcribe recording.wav --backend whisper --language tr --model /path/to/ggml-large-v3.bin
```

Dosyadan metne çevirme de kayıtlarla aynı gürültü azaltma ayarını kullanır. Karşılaştırmak için yalnızca o komutta kapatabilirsiniz:

```sh
just-whisper --transcribe recording.wav --language tr --no-noise-reduction
```

Elle uçtan uca kontrol: bir metin alanına odaklanın, global kısayolu basılı tutup Türkçe konuşun, bırakın. Başlangıç kelimelerinin korunmasını, pill'in yuvarlağa dönüşmesini ve metnin aktif alana yapışmasını kontrol edin. Aynı testi model yükleme ayarı açık ve kapalıyken tekrarlayın. Terminalde `Ctrl + Shift + V` seçin. ydotoold durdurulursa sonuç panoda kalır ve arayüz hatayı gösterir.

Kaynaklar: [Whisper modelleri ve GGML dönüştürme](https://github.com/ggml-org/whisper.cpp/tree/master/models), [GlobalShortcuts portalı](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html), [ydotool](https://github.com/ReimuNotMoe/ydotool), [GNOME layer-shell desteği](https://github.com/wmww/gtk4-layer-shell#supported-desktops).
