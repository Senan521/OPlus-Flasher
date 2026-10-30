# OPlus Flasher (Android / OTG)
Tauri 2 + Svelte 5, Material 3 Expressive arayüz. Hedef cihaz OTG ile bağlanır; fastboot ve ADB protokolleri Rust’ta (nusb) uygulanır.

## Derleme
Ayrıntılı adımlar: **README-CACHYOS.md** (Linux). Kısaca:
1. `npm install`
2. `npm run tauri android init`
3. `bash scripts/setup-android.sh`  (Kotlin eklentisi + manifest izinleri + servis)
4. `npm run tauri android build -- --debug --apk --target aarch64`

## İlk açılış
- **Tüm dosyalara erişim** ve **arka planda çalışma (pil optimizasyonu + bildirim)** izinleri istenir; verilmezse üstte uyarı kartı kalır.
- İşlem sürerken (flaş, payload, yedek) uygulama bir ön plan servisi + wake lock ile canlı tutulur.
- Dil: Ayarlar > Genel > Dil.

## Özellikler
- Fastboot: cihaz algılama, bölüm flaşlama (sparse bölme dahil), verileri silme, vbmeta doğrulamasını kapatma.
- Payload: tam OTA (.zip veya payload.bin), çok çekirdekli çıkarma.
- ADB-over-USB: RSA yetkilendirme, shell, root denetimi (Yedek sekmesinde), yeniden başlatma.
- Yedekle/Geri yükle (root): APK + data/de/ext; geri yüklemede kurulu uygulama varsa APK atlanır. Cache klasörleri yedeğe alınmaz.
- KernelSU yama (Flaşla > KernelSU): `ksud` ve `.ko` GitHub’dan indirilir, **hedef cihaza ADB ile gönderilip orada çalıştırılır**, yamalı imaj geri alınır; sonra fastboot ile flaşlanır. (Uygulamaya `libksud.so` gömmek gerekmez.)
- Önemli: hedef cihaz KernelSU için GKI (KMI) olmalı; yamalamadan önce orijinal `init_boot/boot` imajını yedekle.

## Sorun bildirirken
Ayarlar > Günlük > Kopyala çıktısını ekle. ADB hataları son paket geçmişini (`→OPEN …`, `←CLSE …`) içerir.
