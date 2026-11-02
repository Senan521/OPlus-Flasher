// Anahtar = Türkçe metin. İngilizce çeviri yoksa Türkçe metin gösterilir.
export const EN = {
  'Cihaz':'Device','Flaşla':'Flash','Payload':'Payload','Yedek':'Backup','Ayarlar':'Settings','Yedekle':'Backup','Günlük':'Log',
  'OTG ile hedef cihazı bağla':'Connect the target device via OTG','{m} bağlı (ADB)':'{m} connected (ADB)','{p} bağlı':'{p} connected',
  'Dosya erişim izni gerekli':'File access permission required','ROM, payload ve yedek dosyalarını okumak için “Tüm dosyalara erişim” iznini ver.':'Grant “All files access” so ROM, payload and backup files can be read.','İzin ver':'Allow',
  'Fastboot cihazı':'Fastboot device','Cihaz bağlı değil':'No device connected','Seri: {s}':'Serial: {s}','Telefonu OTG kablosuyla tak, otomatik algılanır.':'Plug the phone in with an OTG cable; it is detected automatically.',
  'Bağlantıyı kes':'Disconnect','Yeniden tara':'Rescan','Android {v}':'Android {v}','Root: {r}':'Root: {r}','yok':'none','MODEL':'MODEL','SERİ':'SERIAL','Root durumunu denetle':'Check root status',
  'Sistem':'System','Bootloader':'Bootloader','Fastbootd':'Fastbootd','Recovery':'Recovery','ADB shell komutu':'ADB shell command','Çalıştır':'Run',
  'Bootloader {s}':'Bootloader {s}','açık':'unlocked','kilitli':'locked','Slot {s}':'Slot {s}','ÜRÜN':'PRODUCT','MAX İNDİRME':'MAX DOWNLOAD','Bağlan':'Connect','Desteklenmiyor':'Unsupported',
  'İmajlar':'Images','KernelSU':'KernelSU','KernelSU yama':'KernelSU patch','ksud boot-patch · yalnızca GKI cihazlar':'ksud boot-patch · GKI devices only','ksud hazır':'ksud ready','ksud yok':'ksud missing',
  '{e}. KernelSU sürümündeki aarch64 Android ksud dosyasını jniLibs/arm64-v8a/libksud.so olarak ekleyip yeniden derle (README).':'{e}. Add the aarch64 Android ksud from a KernelSU release as jniLibs/arm64-v8a/libksud.so and rebuild (README).',
  'GitHub deposu':'GitHub repository','Sürümleri getir':'Fetch releases','{n} KMI':'{n} KMI','ön sürüm':'pre-release','KMI (örn. android14-6.1)':'KMI (e.g. android14-6.1)','KMI’yi algıla':'Detect KMI','Yamala':'Patch',
  'Klasörde: {l}':'In folder: {l}','{f} içinde imaj yok':'No images in {f}','Yamalı imaj hazır':'Patched image ready','vbmeta doğrulamasını kapat':'Disable vbmeta verification','klasörde vbmeta.img varsa':'if vbmeta.img is in the folder',
  'Sonra yeniden başlat':'Reboot afterwards','Flaşlamak için cihaz fastboot modunda bağlı olmalı.':'The device must be connected in fastboot mode to flash.','Yöneticiyi kur (ADB)':'Install manager (ADB)',
  'ROM klasörü (.img dosyaları)':'ROM folder (.img files)','Klasör seç':'Choose folder','Dosya seç':'Choose file','İmajları tara':'Scan images','Hazırlanıyor':'Preparing','{n} imaj · {m} seçili':'{n} images · {m} selected',
  'Seçimi kaldır':'Deselect all','Tümünü seç':'Select all','boot zinciri':'boot chain','Flaşla ({n})':'Flash ({n})',
  'Payload Dumper':'Payload Dumper','OTA .zip veya payload.bin → .img (super atlanır)':'OTA .zip or payload.bin → .img (super is skipped)','OTA zip / payload.bin yolu':'OTA zip / payload.bin path','Çıktı klasörü: {f}':'Output folder: {f}',
  'Bölümleri oku':'Read partitions','Durdur':'Stop','{n} bölüm · {m} seçili':'{n} partitions · {m} selected','Seçilenleri çıkar':'Extract selected',
  'Yedekle / Geri Yükle':'Backup / Restore','ADB + root · hedef cihazdaki uygulamalar':'ADB + root · apps on the target device','Yedek klasörü (bu telefonda)':'Backup folder (on this phone)','Uygulamalar':'Apps','Yedekler':'Backups','Paket adı ara':'Search package name',
  'Yedeklemek için önce Cihaz sekmesinden ADB ile bağlan.':'Connect via ADB from the Device tab first.','Uygulamalar: {n} · {m} seçili':'Apps: {n} · {m} selected','Seçilenleri yedekle':'Back up selected','Yedekler: {n} · {m} seçili':'Backups: {n} · {m} selected','Seçilenleri geri yükle':'Restore selected',
  '{n} MB · {d}':'{n} MB · {d}','Genel':'General','Görünüm':'Appearance','Açık':'Light','Koyu':'Dark','Dil':'Language','Türkçe':'Türkçe','English':'English','Bağlantı':'Connection','Otomatik bağlan':'Auto-connect',
  'Fastboot/ADB cihazı takılınca kendiliğinden bağlan':'Connect automatically when a fastboot/ADB device is plugged in','Hakkında':'About','OPlus Flasher 0.3 · Material 3 Expressive':'OPlus Flasher 0.3 · Material 3 Expressive','ADB anahtarı uygulama verisinde saklanır.':'The ADB key is stored in the app data.',
  'Günlük ({n})':'Log ({n})','Kopyala':'Copy','Temizle':'Clear','Henüz kayıt yok.':'No log entries yet.',
  '{n} bölüm flaşlanacak':'{n} partitions will be flashed','Yanlış imaj cihazı bozabilir. Kabloyu çıkarma, uygulamayı kapatma ve ekranı açık tut.':'A wrong image can brick the device. Do not unplug the cable or close the app, and keep the screen on.',
  'Yedeğim var, riski anlıyorum':'I have a backup and understand the risk','Vazgeç':'Cancel','Başlat':'Start',
  'Flaşlama tamamlandı':'Flashing complete','Yeni bir ROM’a geçtiysen verileri silmen (format data) gerekebilir. Bu işlem dahili depolamadaki her şeyi siler.':'If you switched to a new ROM you may need to wipe data (format data). This erases everything on internal storage.',
  'Her şeyin silineceğini anlıyorum':'I understand everything will be erased','Şimdilik değil':'Not now','Yeniden başlat':'Reboot','Verileri sil':'Wipe data',
  'Arka planda çalışma izni gerekli':'Background permission required','Flaşlama, yedekleme ve payload işlemleri uygulama arka plana alınınca durabilir. Pil optimizasyonunu kapat ve bildirimlere izin ver.':'Flashing, backup and payload jobs can stop when the app goes to the background. Disable battery optimization and allow notifications.',
  'Arka planda çalışma izni yok: uygulama arka plana alınırsa işlem durabilir.':'No background permission: the job may stop if the app goes to the background.','OTA zip dosyası veya Payload.bin dosyasını seçin':'Select an OTA zip file or a payload.bin file',
  'ADB Shell Root iznini verdiğinizden emin olun':'Make sure you have granted ADB Shell root access','ADB bağlı':'ADB connected','ADB gerekli':'ADB required',
  'Yama hedef cihazda yapılır: hedef cihaz ADB ile bağlı olmalı (ksud cihaza gönderilip orada çalıştırılır).':'Patching runs on the target device: it must be connected via ADB (ksud is sent to the device and executed there).',
  'KMI’ler ksud içinde':'KMIs are built into ksud','Yönetici APK’sı':'Manager APK','Seçili sürümün yönetici uygulamasını hedef cihaza ADB ile kur.':'Install the selected release’s manager app on the target device via ADB.',
  'Normal sürüm':'Normal build','Spoofed sürüm':'Spoofed build','APK bulunamadı':'APK not found','spoofed APK yok':'no spoofed APK','Kurmak için hedef cihaz ADB ile bağlı olmalı.':'The target device must be connected via ADB to install.',
  'Telefonda dil seçimi çıkarsa English seç':'If a language prompt appears on the phone, choose English','Genel':'General','Stock ROM':'Stock ROM','AOSP':'AOSP','bootloader · A/B':'bootloader · A/B','fastbootd · A':'fastbootd · A only','fastbootd · mantıksal (A)':'fastbootd · logical (A)',
  'Mantıksal bölümler silinip yeniden oluşturulur; B slotundaki kopyaları boş kalır. İşlem sonunda aktif slot A yapılır.':'Logical partitions are deleted and recreated; their B-slot copies stay empty. The active slot is set to A at the end.',
  'Stock ROM (OnePlus 13 · OOS): bootloader’da A/B, fastbootd’de yalnızca A, sonra set_active a.':'Stock ROM (OnePlus 13 · OOS): A/B in bootloader, only A in fastbootd, then set_active a.',
  'AOSP (OnePlus 13): bootloader’da A/B, fastbootd’de yalnızca A, sonra set_active a.':'AOSP (OnePlus 13): A/B in bootloader, only A in fastbootd, then set_active a.',
  'Genel: A/B cihazlarda tüm imajlar iki slota yazılır.':'General: on A/B devices every image is written to both slots.',
  'SSAID’i de geri yükle':'Also restore SSAID','Sistem dosyasını değiştirir (riskli); sonra cihazı yeniden başlat':'Modifies a system file (risky); reboot the device afterwards',
  'ADB Sideload':'ADB Sideload','ROM veya başka bir zip dosyasını Recovery üzerinden yükle':'Install a ROM or any other zip through Recovery','Sideload modunda':'In sideload mode','Sideload modunda değil ({s})':'Not in sideload mode ({s})','Denetlenmedi':'Not checked',
  'Hedef cihazda Recovery → Apply update → Apply from ADB seç, sonra aşağıdan denetle.':'On the target device choose Recovery → Apply update → Apply from ADB, then check below.','Sideload modunu denetle':'Check sideload mode','Recovery’ye yeniden başlat':'Reboot to Recovery',
  'Zip dosyası yolu':'Zip file path','Sideload ile yükle':'Install via sideload','Önce cihazın sideload modunda olduğunu denetle.':'First check that the device is in sideload mode.','ADB cihazı bulunamadı':'No ADB device found','ADB durumu: {s}':'ADB state: {s}',
  'Sideload denetleniyor':'Checking sideload','Sideload gönderiliyor':'Sending via sideload','Sideload tamamlandı':'Sideload finished','Extra':'Extra',
  // busy labels
  'Bağlanılıyor':'Connecting','Root denetleniyor':'Checking root','Komut çalışıyor':'Running command','Yeniden başlatılıyor':'Rebooting','Taranıyor':'Scanning','Flaşlanıyor':'Flashing','Veriler siliniyor':'Wiping data',
  'Payload okunuyor':'Reading payload','Çıkarılıyor':'Extracting','Uygulamalar okunuyor':'Reading apps','Yedekler okunuyor':'Reading backups','Yedekleniyor':'Backing up','Geri yükleniyor':'Restoring',
  'KernelSU hazırlanıyor':'Preparing KernelSU','Sürümler alınıyor':'Fetching releases','KMI algılanıyor':'Detecting KMI','Yamalanıyor':'Patching','KSU flaşlanıyor':'Flashing KSU','Yönetici kuruluyor':'Installing manager',
  // log lines
  'Cihaz bağlantısı kesildi':'Device disconnected','USB cihazı algılandı: {p}':'USB device detected: {p}','Hedef cihazda “USB hata ayıklamaya izin ver” iletisi çıkarsa onayla':'If the “Allow USB debugging” prompt appears on the target device, accept it',
  'ADB bağlandı: {m}':'ADB connected: {m}','Bağlandı: {p} · {s}':'Connected: {p} · {s}','(çıktı yok)':'(no output)','{n} imaj bulundu':'{n} images found','Fastbootd’a geçiliyor…':'Switching to fastbootd…',
  'Flaşlama tamamlandı.':'Flashing complete.','{n} bölüm bulundu':'{n} partitions found','Çıktı: {p}':'Output: {p}','{n} kullanıcı uygulaması':'{n} user apps','{n} yedek bulundu':'{n} backups found','Yedekleme bitti':'Backup finished','Geri yükleme bitti':'Restore finished',
  'Hedef cihazın ekranında USB hata ayıklama iznini onayla':'Accept the USB debugging prompt on the target device screen','Günlük panoya kopyalandı':'Log copied to clipboard','pano erişimi yok':'clipboard not available',
  '{n} sürüm bulundu':'{n} releases found','KMI: {k} ({s})':'KMI: {k} ({s})','KMI algılanamadı, elle seç':'KMI not detected, choose manually','Yamalı imaj: {p}':'Patched image: {p}',
  'İpucu: Android “Tüm dosyalara erişim” iznini vermelisin (üstteki “İzin ver” kartı).':'Hint: grant Android “All files access” (use the “Allow” card at the top).','HATA':'ERROR','Durduruluyor…':'Stopping…',
};
// Arka uçtan gelen Türkçe hata parçaları
export const ERR = {
  'cihaz bağlı değil':'device not connected','ADB cihazı bağlı değil':'ADB device not connected','seri numarası değişti, işlem iptal edildi':'serial number changed, aborted','super bölümü bilerek atlanıyor':'the super partition is skipped on purpose',
  'iptal edildi':'cancelled','Seçim iptal edildi':'Selection cancelled','USB izni reddedildi':'USB permission denied','Aygıt bulunamadı':'Device not found','servis reddedildi':'service rejected',
  'Root doğrulanamadı':'Could not verify root','Magisk/KernelSU’da Shell’e root izni ver, hedef ekranı açık tut.':'Grant Shell root access in Magisk/KernelSU and keep the target screen on.','Uygulama listesi boş. Çıktı:':'App list is empty. Output:',
  'ADB yetkilendirme zaman aşımı (hedef cihazda izni onayla ve yeniden dene)':'ADB authorization timed out (accept the prompt on the target device and retry)','USB hata ayıklama izni verilmedi':'USB debugging was not authorized',
  'geçerli bir payload.bin değil':'not a valid payload.bin','zip içinde payload.bin yok':'no payload.bin inside the zip','desteklenmeyen işlem türü':'unsupported operation type','artımlı OTA desteklenmez, tam OTA kullan':'incremental OTAs are not supported, use a full OTA',
  'geçersiz bölüm adı':'invalid partition name','imaj max-download-size sınırını aşıyor':'image exceeds max-download-size','Uygun USB arayüzü yok':'No suitable USB interface','Bu konumdan dosya yolu alınamadı; Dahili depolamadan seç':'Could not get a file path from this location; pick from internal storage',
};
import { RU, ERR_RU } from './ru.js';
export function makeT(getLang) {
  const t = (s, p) => {
    const l = getLang();
    let r = l === 'en' ? (EN[s] ?? s) : l === 'ru' ? (RU[s] ?? EN[s] ?? s) : s;
    if (p) for (const k in p) r = r.split(`{${k}}`).join(p[k]);
    return r;
  };
  const tr = (m) => {
    let r = String(m);
    const l = getLang();
    if (l === 'en') for (const k in ERR) r = r.split(k).join(ERR[k]);
    else if (l === 'ru') for (const k in ERR) r = r.split(k).join(ERR_RU[k] ?? ERR[k]);
    return r;
  };
  return { t, tr };
}
