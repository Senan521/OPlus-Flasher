#!/usr/bin/env bash
# `npm run tauri android init` sonrasında bir kez çalıştır (tekrar çalıştırmak güvenlidir).
set -euo pipefail
cd "$(dirname "$0")/.."
G=src-tauri/gen/android/app/src/main
[ -d "$G" ] || { echo "Önce: npm run tauri android init"; exit 1; }
mkdir -p "$G/java/dev/linuxflasher/usb" "$G/res/xml"
cp android-src/app/src/main/java/dev/linuxflasher/usb/*.kt "$G/java/dev/linuxflasher/usb/"
cp android-src/app/src/main/res/xml/device_filter.xml "$G/res/xml/"
# `tauri android init` varsayılan Tauri ikonlarını üretir; kendi ikonlarımız üzerine yazılır.
if [ -d src-tauri/icons/android ]; then
  find "$G/res" -name 'ic_launcher*.webp' -delete 2>/dev/null || true
  cp -r src-tauri/icons/android/. "$G/res/"
  echo "Uygulama ikonu kopyalandı: src-tauri/icons/android -> $G/res"
else
  echo "UYARI: src-tauri/icons/android yok, varsayılan ikon kalır"
fi
python3 - "$G/AndroidManifest.xml" <<'PY'
import sys
p = sys.argv[1]; s = open(p, encoding='utf-8').read()
perms = [
 ('MANAGE_EXTERNAL_STORAGE', ''), ('READ_EXTERNAL_STORAGE', ' android:maxSdkVersion="29"'), ('WRITE_EXTERNAL_STORAGE', ' android:maxSdkVersion="29"'),
 ('FOREGROUND_SERVICE', ''), ('FOREGROUND_SERVICE_DATA_SYNC', ''), ('POST_NOTIFICATIONS', ''),
 ('REQUEST_IGNORE_BATTERY_OPTIMIZATIONS', ''), ('WAKE_LOCK', ''), ('INTERNET', ''),
]
add = ''
for n, extra in perms:
    if f'android.permission.{n}"' not in s:
        add += f'    <uses-permission android:name="android.permission.{n}"{extra} />\n'
if 'android.hardware.usb.host' not in s:
    add += '    <uses-feature android:name="android.hardware.usb.host" android:required="true" />\n'
if add: s = s.replace('<application', add + '    <application', 1)
if 'KeepAliveService' not in s:
    s = s.replace('</application>', '        <service android:name="dev.linuxflasher.usb.KeepAliveService" android:foregroundServiceType="dataSync" android:exported="false" />\n    </application>', 1)
if 'USB_DEVICE_ATTACHED' not in s:
    s = s.replace('</activity>', '        <intent-filter><action android:name="android.hardware.usb.action.USB_DEVICE_ATTACHED" /></intent-filter>\n        <meta-data android:name="android.hardware.usb.action.USB_DEVICE_ATTACHED" android:resource="@xml/device_filter" />\n    </activity>', 1)
open(p, 'w', encoding='utf-8').write(s)
print('Manifest güncellendi:', p)
PY
echo "Tamam. Derle: npm run tauri android build -- --debug --apk --target aarch64"
