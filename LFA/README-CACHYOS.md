# CachyOS’ta derleme (APK)
CachyOS Arch tabanlıdır; varsayılan kabuğu **fish**’tir. Aşağıda fish ve bash komutları ayrı verildi.

## 1) Sistem paketleri
```
sudo pacman -Syu --needed base-devel git curl wget unzip jdk17-openjdk nodejs npm rustup python
sudo archlinux-java set java-17-openjdk
rustup default stable
rustup target add aarch64-linux-android
```
(Eski/32-bit telefonlar için ayrıca: `armv7-linux-androideabi i686-linux-android x86_64-linux-android`.)

## 2) Android SDK + NDK (komut satırı, Android Studio gerekmez)
1. https://developer.android.com/studio#command-line-tools-only adresinden “Command line tools only” Linux zip’ini indir.
2. ```
   mkdir -p ~/Android/Sdk/cmdline-tools
   unzip ~/Downloads/commandlinetools-linux-*.zip -d ~/Android/Sdk/cmdline-tools
   mv ~/Android/Sdk/cmdline-tools/cmdline-tools ~/Android/Sdk/cmdline-tools/latest
   ```
3. Ortam değişkenleri (**fish**):
   ```
   set -Ux JAVA_HOME /usr/lib/jvm/java-17-openjdk
   set -Ux ANDROID_HOME $HOME/Android/Sdk
   fish_add_path $ANDROID_HOME/cmdline-tools/latest/bin $ANDROID_HOME/platform-tools
   ```
   **bash/zsh** (`~/.bashrc`): 
   ```
   export JAVA_HOME=/usr/lib/jvm/java-17-openjdk
   export ANDROID_HOME=$HOME/Android/Sdk
   export PATH=$PATH:$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/platform-tools
   ```
4. Lisansları kabul et ve paketleri kur (sürümleri `sdkmanager --list` çıktısından seç; en yeni kararlı platform/build-tools ve bir NDK yeterli):
   ```
   yes | sdkmanager --licenses
   sdkmanager --list | grep -E "platforms;android-|build-tools;|ndk;"
   sdkmanager "platform-tools" "platforms;android-<N>" "build-tools;<N>.0.0" "ndk;<SÜRÜM>"
   ```
5. NDK yolu (**fish**): `set -Ux NDK_HOME (ls -d $ANDROID_HOME/ndk/* | tail -1)`  
   (**bash**): `export NDK_HOME="$(ls -d $ANDROID_HOME/ndk/* | tail -1)"`
6. Yeni terminal aç; `echo $JAVA_HOME $ANDROID_HOME $NDK_HOME` üçünü de göstermeli.

## 3) Projeyi hazırla
```
unzip OPlusFlasher-Android.zip && cd LFA
npm install
npm run tauri android init
bash scripts/setup-android.sh
```
`setup-android.sh` Kotlin eklentisini, manifest izinlerini ve ön plan servisini otomatik ekler.

## 4) Derle
Hızlı, kendi telefonun için (debug imzalı, hemen kurulur):
```
npm run tauri android build -- --debug --apk --target aarch64
```
APK: `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`  
(İlk derleme Gradle + Rust bağımlılıkları yüzünden 10–30 dk sürebilir, ~2 GB indirir.)

Release imzalamak istersen:
```
keytool -genkey -v -keystore ~/oplus.jks -alias oplus -keyalg RSA -keysize 2048 -validity 10000
npm run tauri android build -- --apk --target aarch64
$ANDROID_HOME/build-tools/<N>.0.0/apksigner sign --ks ~/oplus.jks --out OPlusFlasher.apk \
  src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk
```

## 5) Telefona kur
USB hata ayıklama açıkken `adb install -r <apk>` veya APK’yı telefona kopyalayıp aç (“bilinmeyen kaynaklardan kur” izni gerekir).

## Sık sorunlar
- `JAVA_HOME`/`NDK_HOME` boş: 2. adımdaki değişkenleri yeniden kontrol et, terminali yeniden aç.
- `failed to run custom build command for ring/lzma-sys/zstd-sys`: `NDK_HOME` yanlış ya da NDK kurulu değil.
- `src-tauri/gen/android` bozulduysa sil ve `android init` + `setup-android.sh` adımlarını tekrarla.
- Ayrıntılı çıktı: komutun sonuna `-v` ekle.
