mod adb;
mod backup;
mod fastboot;
mod ksu;
mod payload;
mod usb;
use fastboot::Fastboot;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Wry};

struct Sess { fb: Fastboot, serial: String, max_dl: u64 }
#[derive(Default)]
struct State(Mutex<Option<Sess>>);
type U = usb::Usb<Wry>;

fn hex(s: &str) -> u64 { u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).unwrap_or(0) }
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Info { serial: String, product: String, slot_count: String, current_slot: String, userspace: bool, unlocked: String, max_dl: u64 }
#[derive(Serialize)]
struct Img { name: String, size: u64 }

#[tauri::command]
async fn usb_list(app: AppHandle) -> Result<Vec<usb::UsbDev>, String> {
    blocking(move || app.state::<U>().list()).await
}

#[tauri::command]
async fn usb_connect(app: AppHandle, name: String) -> Result<Info, String> {
    blocking(move || {
        let o = app.state::<U>().open(&name, 3)?;
        let fb = Fastboot::open(o.fd, o.iface, o.ep_in, o.ep_out)?;
        let serial = fb.getvar("serialno");
        let max_dl = hex(&fb.getvar("max-download-size"));
        let info = Info { serial: serial.clone(), product: fb.getvar("product"), slot_count: fb.getvar("slot-count"),
            current_slot: fb.getvar("current-slot"), userspace: fb.getvar("is-userspace") == "yes", unlocked: fb.getvar("unlocked"), max_dl };
        *app.state::<State>().0.lock().unwrap() = Some(Sess { fb, serial, max_dl });
        Ok(info)
    }).await
}

#[tauri::command]
fn usb_disconnect(app: AppHandle) { *app.state::<State>().0.lock().unwrap() = None; }

#[tauri::command]
async fn fb_command(app: AppHandle, cmd: String) -> Result<String, String> {
    const OK: [&str; 3] = ["reboot", "reboot-bootloader", "reboot-fastboot"];
    let slot = cmd.strip_prefix("set_active:").map(|s| s == "a" || s == "b").unwrap_or(false);
    if !OK.contains(&cmd.as_str()) && !slot { return Err("komuta izin verilmiyor".into()); }
    blocking(move || {
        let st = app.state::<State>();
        let mut g = st.0.lock().unwrap();
        let r = g.as_ref().ok_or("cihaz bağlı değil")?.fb.cmd(&cmd)?;
        if cmd.starts_with("reboot") { *g = None; }
        Ok(r)
    }).await
}

#[tauri::command]
fn scan_images(path: String) -> Result<Vec<Img>, String> {
    let mut v: Vec<Img> = std::fs::read_dir(&path).map_err(|e| format!("{path}: {e}"))?
        .filter_map(|r| r.ok()).filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.strip_suffix(".img").filter(|s| *s != "super").map(|s| Img { name: s.to_string(), size: e.metadata().map(|m| m.len()).unwrap_or(0) })
        }).collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(v)
}

#[tauri::command]
async fn flash_image(app: AppHandle, partition: String, path: String) -> Result<String, String> {
    if partition.starts_with("super") { return Err("super bölümü bilerek atlanıyor".into()); }
    if partition.is_empty() || !partition.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err("geçersiz bölüm adı".into());
    }
    blocking(move || {
        let st = app.state::<State>();
        let g = st.0.lock().unwrap();
        let s = g.as_ref().ok_or("cihaz bağlı değil")?;
        if s.fb.getvar("serialno") != s.serial { return Err("seri numarası değişti, işlem iptal edildi".to_string()); }
        let size = std::fs::metadata(&path).map_err(|e| format!("{path}: {e}"))?.len();
        let (a, p) = (app.clone(), partition.clone());
        let mut last = std::time::Instant::now();
        let mut cb = |d: u64, t: u64| { if d >= t || last.elapsed().as_millis() >= 150 { last = std::time::Instant::now(); let _ = a.emit("flash-progress", (p.as_str(), d, t)); } };
        if s.max_dl > 0 && size > s.max_dl { s.fb.flash_split(&partition, &path, s.max_dl, &mut cb)?; }
        else { s.fb.flash(&partition, &path, &mut cb)?; }
        Ok(format!("{partition} ✓"))
    }).await
}

#[derive(Default)]
struct AdbS(Mutex<Option<adb::Adb>>);
#[derive(Serialize)]
struct AdbInfo { model: String, android: String, serial: String }

#[tauri::command]
async fn adb_connect(app: AppHandle, name: String) -> Result<AdbInfo, String> {
    blocking(move || {
        let o = app.state::<U>().open(&name, 1)?;
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // Yetkilendirme penceresi yanıtsız kalırsa arayüz sonsuza dek kilitlenmesin
        let (tx, rx) = std::sync::mpsc::channel();
        let (ap, kp) = (app.clone(), dir.join("adbkey.der"));
        std::thread::spawn(move || {
            let r = adb::Adb::open(o.fd, o.iface, o.ep_in, o.ep_out, o.mps, &kp, &move || { let _ = ap.emit("adb-auth", ()); });
            let _ = tx.send(r);
        });
        let mut a = rx.recv_timeout(std::time::Duration::from_secs(90))
            .map_err(|_| "ADB yetkilendirme zaman aşımı (hedef cihazda izni onayla ve yeniden dene)".to_string())??;
        let mut gp = |k: &str| a.exec(&format!("getprop {k}")).unwrap_or_default().trim().to_string();
        let info = AdbInfo { model: gp("ro.product.model"), android: gp("ro.build.version.release"), serial: gp("ro.serialno") };
        *app.state::<AdbS>().0.lock().unwrap() = Some(a);
        Ok(info)
    }).await
}

#[tauri::command]
fn adb_disconnect(app: AppHandle) { *app.state::<AdbS>().0.lock().unwrap() = None; }

#[tauri::command]
async fn adb_exec(app: AppHandle, cmd: String) -> Result<String, String> {
    blocking(move || {
        let st = app.state::<AdbS>();
        let mut g = st.0.lock().unwrap();
        g.as_mut().ok_or("ADB cihazı bağlı değil")?.exec(&cmd)
    }).await
}

#[tauri::command]
async fn adb_root_check(app: AppHandle) -> Result<String, String> { blocking(move || with_adb(&app, |a| backup::check_root(a))).await }

#[tauri::command]
async fn adb_reboot(app: AppHandle, target: String) -> Result<(), String> {
    if !["", "bootloader", "fastboot", "recovery"].contains(&target.as_str()) { return Err("geçersiz hedef".into()); }
    blocking(move || {
        let st = app.state::<AdbS>();
        let mut g = st.0.lock().unwrap();
        let _ = g.as_mut().ok_or("ADB cihazı bağlı değil")?.exec(&format!("reboot {target}"));
        *g = None;
        Ok(())
    }).await
}

pub(crate) fn with_adb<T>(app: &AppHandle, f: impl FnOnce(&mut adb::Adb) -> Result<T, String>) -> Result<T, String> {
    let st = app.state::<AdbS>();
    let mut g = st.0.lock().unwrap();
    f(g.as_mut().ok_or("ADB cihazı bağlı değil")?)
}
#[tauri::command]
async fn bk_packages(app: AppHandle) -> Result<Vec<String>, String> { blocking(move || with_adb(&app, |a| backup::packages(a))).await }
#[tauri::command]
async fn bk_backup(app: AppHandle, pkgs: Vec<String>, out: String) -> Result<(), String> {
    blocking(move || { let ap = app.clone(); with_adb(&app, |a| backup::backup(&ap, a, &pkgs, &out)) }).await
}
#[tauri::command]
async fn bk_restore(app: AppHandle, pkgs: Vec<String>, out: String) -> Result<(), String> {
    blocking(move || { let ap = app.clone(); with_adb(&app, |a| backup::restore(&ap, a, &pkgs, &out)) }).await
}
#[tauri::command]
fn bk_list(out: String) -> Result<Vec<backup::Item>, String> { backup::list(&out) }
#[tauri::command]
fn bk_cancel() { backup::CANCEL.store(true, std::sync::atomic::Ordering::SeqCst); }

#[tauri::command]
async fn payload_list(path: String) -> Result<Vec<payload::PartInfo>, String> { blocking(move || payload::list(&path)).await }
#[tauri::command]
async fn payload_extract(app: AppHandle, path: String, out: String, names: Vec<String>) -> Result<String, String> {
    blocking(move || payload::extract(&app, &path, &out, &names)).await
}
#[tauri::command]
fn payload_cancel() { payload::CANCEL.store(true, std::sync::atomic::Ordering::SeqCst); }

#[tauri::command]
async fn ksu_detect_kmi(app: AppHandle, folder: String) -> Result<serde_json::Value, String> {
    blocking(move || { let u = with_adb(&app, |a| a.exec("uname -r")).ok(); Ok(ksu::detect_kmi(u, &folder)) }).await
}
#[tauri::command]
async fn ksu_install_manager(app: AppHandle, repo: String, tag: String, spoofed: bool) -> Result<String, String> {
    blocking(move || { let apk = ksu::fetch_manager(&app, &repo, &tag, spoofed)?; with_adb(&app, |a| ksu::install_apk(a, &apk)) }).await
}
#[tauri::command]
async fn ksu_flash(app: AppHandle, partition: String, file: String, vbmeta: Option<String>, reboot: bool) -> Result<String, String> {
    if partition != "init_boot" && partition != "boot" { return Err("bölüm init_boot veya boot olmalı".into()); }
    blocking(move || {
        let st = app.state::<State>();
        let mut g = st.0.lock().unwrap();
        let s = g.as_ref().ok_or("cihaz bağlı değil")?;
        if s.fb.getvar("serialno") != s.serial { return Err("seri numarası değişti, işlem iptal edildi".to_string()); }
        let slot = if s.fb.getvar("slot-count") == "2" { format!("_{}", s.fb.getvar("current-slot").trim_start_matches('_')) } else { String::new() };
        let flash = |part: &str, path: &str| -> Result<(), String> {
            let size = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?.len();
            let (a, p) = (app.clone(), part.to_string());
            let mut last = std::time::Instant::now();
            let mut cb = |d: u64, t: u64| { if d >= t || last.elapsed().as_millis() >= 150 { last = std::time::Instant::now(); let _ = a.emit("flash-progress", (p.as_str(), d, t)); } };
            if s.max_dl > 0 && size > s.max_dl { s.fb.flash_split(part, path, s.max_dl, &mut cb) } else { s.fb.flash(part, path, &mut cb) }
        };
        flash(&format!("{partition}{slot}"), &file)?;
        let mut msg = format!("{partition}{slot} ✓");
        if let Some(vb) = vbmeta {
            let tmp = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("vbmeta_noverity.img");
            ksu::disable_verity(std::path::Path::new(&vb), &tmp)?;
            flash(&format!("vbmeta{slot}"), &tmp.to_string_lossy())?;
            msg += " · vbmeta (doğrulama kapalı) ✓";
        }
        if reboot { let _ = s.fb.cmd("reboot"); *g = None; msg += " · yeniden başlatılıyor"; }
        Ok(msg)
    }).await
}

#[tauri::command]
async fn bg_granted(app: AppHandle) -> Result<bool, String> { blocking(move || app.state::<U>().bg_granted()).await }
#[tauri::command]
async fn bg_request(app: AppHandle) -> Result<(), String> { blocking(move || app.state::<U>().bg_request()).await }
#[tauri::command]
async fn keep_alive(app: AppHandle, on: bool) -> Result<(), String> { blocking(move || app.state::<U>().keep_alive(on)).await }

#[tauri::command]
async fn storage_granted(app: AppHandle) -> Result<bool, String> { blocking(move || app.state::<U>().storage_granted()).await }
#[tauri::command]
async fn storage_request(app: AppHandle) -> Result<(), String> { blocking(move || app.state::<U>().storage_request()).await }

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<String, String> { blocking(move || app.state::<U>().pick("pickFolder")).await }
#[tauri::command]
async fn pick_file(app: AppHandle) -> Result<String, String> { blocking(move || app.state::<U>().pick("pickFile")).await }

/// fastbootd only: recreates a logical partition the way the OPlus flasher scripts do. Both slots (and their -cow
/// snapshots) are deleted, `_a`/`_b` are created, and `_a` is resized to the image so a normal flash can follow.
#[tauri::command]
async fn fb_logical(app: AppHandle, name: String, path: String) -> Result<String, String> {
    if name.is_empty() || name.starts_with("super") || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return Err("geçersiz bölüm adı".into());
    }
    if !path.ends_with(".img") { return Err("geçersiz imaj yolu".into()); }
    blocking(move || {
        let st = app.state::<State>();
        let g = st.0.lock().unwrap();
        let s = g.as_ref().ok_or("cihaz bağlı değil")?;
        if s.fb.getvar("serialno") != s.serial { return Err("seri numarası değişti, işlem iptal edildi".to_string()); }
        if s.fb.getvar("is-userspace") != "yes" { return Err("mantıksal bölümler için fastbootd gerekli".to_string()); }
        let size = std::fs::metadata(&path).map_err(|e| format!("{path}: {e}"))?.len();
        for sfx in ["a", "b"] {
            let _ = s.fb.cmd(&format!("delete-logical-partition:{name}_{sfx}"));
            let _ = s.fb.cmd(&format!("delete-logical-partition:{name}_{sfx}-cow"));
        }
        for sfx in ["a", "b"] { s.fb.cmd(&format!("create-logical-partition:{name}_{sfx}:1"))?; }
        s.fb.cmd(&format!("resize-logical-partition:{name}_a:{size}"))?;
        Ok(format!("{name}_a hazır ({size} bayt), {name}_b boş"))
    }).await
}

#[tauri::command]
async fn fb_wipe(app: AppHandle) -> Result<String, String> {
    blocking(move || {
        let st = app.state::<State>();
        let g = st.0.lock().unwrap();
        let s = g.as_ref().ok_or("cihaz bağlı değil")?;
        if s.fb.getvar("serialno") != s.serial { return Err("seri numarası değişti, işlem iptal edildi".to_string()); }
        s.fb.cmd("erase:userdata")?;
        Ok(if s.fb.cmd("erase:metadata").is_ok() { "userdata + metadata silindi" } else { "userdata silindi (metadata yok/silinemedi)" }.to_string())
    }).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(usb::init())
        .manage(State::default())
        .manage(AdbS::default())
        .invoke_handler(tauri::generate_handler![usb_list, usb_connect, usb_disconnect, fb_command, scan_images, flash_image, payload_list, payload_extract, payload_cancel, adb_connect, adb_disconnect, adb_exec, adb_root_check, adb_reboot, bk_packages, bk_backup, bk_restore, bk_list, bk_cancel, pick_folder, pick_file, fb_wipe, fb_logical, storage_granted, storage_request, bg_granted, bg_request, keep_alive, ksu_detect_kmi, ksu_install_manager, ksu_flash, ksu::ksu_releases, ksu::ksu_scan, ksu::ksu_patch])
        .run(tauri::generate_context!())
        .expect("error while running Linux Flasher");
}
