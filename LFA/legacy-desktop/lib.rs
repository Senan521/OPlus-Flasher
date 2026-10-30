mod backup;
mod ksu;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter};
use std::{collections::HashMap, sync::{Mutex, OnceLock}};

// Static per-device info (Android version, product name) is queried once, not on every auto-detect poll.
static INFO_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn cached_info(key: &str, fetch: impl FnOnce() -> Option<String>) -> Option<String> {
    let cache = INFO_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().ok().and_then(|c| c.get(key).cloned()) { return Some(v); }
    let value = fetch()?;
    if let Ok(mut c) = cache.lock() { c.insert(key.to_string(), value.clone()); }
    Some(value)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    connected: bool,
    mode: String,
    serial: String,
    product: String,
    model: String,
    android: String,
    slot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageInfo {
    name: String,
    path: String,
    size: u64,
    size_human: String,
    partition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlashStep {
    mode: String,
    image: String,
    partition: String,
    path: String,
    action: String,
    critical: bool,
}

fn run_output(program: &str, args: &[&str]) -> Result<Output, String> {
    let path = find_tool(program).ok_or_else(|| format!("{program} bulunamadı. Ayarlar → Gerekli araçlar bölümünden kurabilirsin."))?;
    Command::new(path)
        .args(args)
        .output()
        .map_err(|e| format!("{program} çalıştırılamadı: {e}"))
}

fn command_text(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    format!("{stdout}{stderr}").trim().to_string()
}

fn human_size(size: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut n = size as f64;
    let mut i = 0;
    while n >= 1024.0 && i < UNITS.len() - 1 { n /= 1024.0; i += 1; }
    if i == 0 { format!("{} {}", size, UNITS[i]) } else { format!("{n:.1} {}", UNITS[i]) }
}

#[tauri::command]
async fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || list_devices_blocking()).await.map_err(|e| e.to_string())?
}

fn list_devices_blocking() -> Result<Vec<DeviceInfo>, String> {
    let mut devices = Vec::new();
    if let Ok(out) = run_output("adb", &["devices", "-l"]) {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let line = line.trim();
                if line.is_empty() { continue; }
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 2 || parts[1] != "device" { continue; }
                let serial = parts[0].to_string();
                let product = parts.iter().find_map(|p| p.strip_prefix("product:")).unwrap_or("unknown").to_string();
                let model = parts.iter().find_map(|p| p.strip_prefix("model:")).unwrap_or("unknown").to_string();
                let android = cached_info(&format!("adb:{serial}:release"), || adb_prop(&serial, "ro.build.version.release")).unwrap_or_else(|| "—".into());
                devices.push(DeviceInfo { connected: true, mode: "ADB / Android".into(), serial, product, model, android, slot: "—".into() });
            }
        }
    }
    if let Ok(out) = run_output("fastboot", &["devices"]) {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.is_empty() { continue; }
                let serial = parts[0].to_string();
                if devices.iter().any(|d| d.serial == serial) { continue; }
                let product = cached_info(&format!("fb:{serial}:product"), || fastboot_var(&serial, "product")).unwrap_or_else(|| "unknown".into());
                let slot = fastboot_var(&serial, "current-slot").unwrap_or_else(|| "unknown".into());
                let userspace = fastboot_var(&serial, "is-userspace").unwrap_or_else(|| "no".into());
                let mode = if userspace.eq_ignore_ascii_case("yes") || userspace == "1" { "Fastbootd" } else { "Bootloader Fastboot" };
                devices.push(DeviceInfo { connected: true, mode: mode.into(), serial, product, model: "—".into(), android: "—".into(), slot });
            }
        }
    }
    Ok(devices)
}

fn adb_prop(serial: &str, key: &str) -> Option<String> {
    let out = run_output("adb", &["-s", serial, "shell", "getprop", key]).ok()?;
    if !out.status.success() { return None; }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

fn fastboot_var(serial: &str, key: &str) -> Option<String> {
    let out = run_output("fastboot", &["-s", serial, "getvar", key]).ok()?;
    let text = command_text(&out);
    // fastboot prints "<key>: <value>" (sometimes behind "(bootloader) ") plus unrelated lines that also contain ':'
    text.lines().find_map(|line| {
        let l = line.trim().trim_start_matches("(bootloader)").trim();
        let rest = l.strip_prefix(key)?.strip_prefix(':')?;
        Some(rest.trim().to_string())
    }).filter(|s| !s.is_empty())
}

#[tauri::command]
async fn choose_rom_folder() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        Ok(rfd::FileDialog::new().set_title("ROM imaj klasörünü seç").pick_folder().map(|p| p.to_string_lossy().into_owned()))
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn choose_zip() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        Ok(rfd::FileDialog::new().set_title("ROM / OTA ZIP seç").add_filter("ROM ZIP", &["zip"]).pick_file().map(|p| p.to_string_lossy().into_owned()))
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn scan_images(path: String) -> Result<Vec<ImageInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || scan_images_blocking(path)).await.map_err(|e| e.to_string())?
}

fn scan_images_blocking(path: String) -> Result<Vec<ImageInfo>, String> {
    let root = PathBuf::from(&path);
    if !root.is_dir() { return Err("Seçilen yol bir klasör değil.".into()); }
    let mut result = Vec::new();
    let entries = fs::read_dir(&root).map_err(|e| format!("Klasör okunamadı: {e}"))?;
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() || p.extension().and_then(|e| e.to_str()).map(|s| !s.eq_ignore_ascii_case("img")).unwrap_or(true) { continue; }
        let metadata = fs::metadata(&p).map_err(|e| e.to_string())?;
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let partition = p.file_stem().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        result.push(ImageInfo { name, path: p.to_string_lossy().into_owned(), size: metadata.len(), size_human: human_size(metadata.len()), partition });
    }
    result.sort_by(|a,b| a.name.cmp(&b.name));
    Ok(result)
}

// ---- Real progress: read partition names/sizes from the payload.bin manifest ----
enum PbVal<'a> { Varint(u64), Bytes(&'a [u8]), Skip }

fn pb_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        let b = *buf.get(*pos)?;
        *pos += 1;
        result |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 { return Some(result); }
        shift += 7;
        if shift >= 64 { return None; }
    }
}

fn pb_fields<'a>(buf: &'a [u8]) -> Option<Vec<(u32, PbVal<'a>)>> {
    let mut pos = 0usize;
    let mut out = Vec::new();
    while pos < buf.len() {
        let key = pb_varint(buf, &mut pos)?;
        let field = (key >> 3) as u32;
        match key & 7 {
            0 => { let v = pb_varint(buf, &mut pos)?; out.push((field, PbVal::Varint(v))); }
            1 => { pos = pos.checked_add(8)?; if pos > buf.len() { return None; } out.push((field, PbVal::Skip)); }
            2 => {
                let len = pb_varint(buf, &mut pos)? as usize;
                let end = pos.checked_add(len)?;
                let slice = buf.get(pos..end)?;
                pos = end;
                out.push((field, PbVal::Bytes(slice)));
            }
            5 => { pos = pos.checked_add(4)?; if pos > buf.len() { return None; } out.push((field, PbVal::Skip)); }
            _ => return None,
        }
    }
    Some(out)
}

/// Returns (partition name, final image size in bytes) for every partition in the payload.
/// Only the small header/manifest is read; `unzip -p` is killed right after.
fn payload_partitions(zip: &Path) -> Option<Vec<(String, u64)>> {
    let unzip = find_tool("unzip")?;
    let mut child = Command::new(unzip).arg("-p").arg(zip).arg("payload.bin")
        .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let parsed = (|| {
        let mut out = child.stdout.take()?;
        let mut head = [0u8; 24];
        out.read_exact(&mut head).ok()?;
        if &head[0..4] != b"CrAU" { return None; }
        let manifest_size = u64::from_be_bytes(<[u8; 8]>::try_from(&head[12..20]).ok()?);
        if manifest_size == 0 || manifest_size > 64 * 1024 * 1024 { return None; }
        let mut manifest = vec![0u8; manifest_size as usize];
        out.read_exact(&mut manifest).ok()?;
        let mut parts = Vec::new();
        for (field, val) in pb_fields(&manifest)? {
            // DeltaArchiveManifest.partitions = 13
            let (13, PbVal::Bytes(p)) = (field, val) else { continue };
            let mut name = String::new();
            let mut size = 0u64;
            for (f, v) in pb_fields(p)? {
                match (f, v) {
                    (1, PbVal::Bytes(b)) => name = String::from_utf8_lossy(b).into_owned(), // partition_name
                    (7, PbVal::Bytes(info)) => {                                            // new_partition_info
                        for (f2, v2) in pb_fields(info)? {
                            if let (1, PbVal::Varint(sz)) = (f2, v2) { size = sz; }          // size
                        }
                    }
                    _ => {}
                }
            }
            if !name.is_empty() && size > 0 { parts.push((name, size)); }
        }
        if parts.is_empty() { None } else { Some(parts) }
    })();
    let _ = child.kill();
    let _ = child.wait();
    parsed
}

/// Bytes actually written so far. If the tool pre-sizes the file (sparse), apparent size is already
/// final, so on Unix the allocated blocks are used instead.
fn written_bytes(path: &Path, expected: u64) -> u64 {
    let Ok(meta) = fs::metadata(path) else { return 0 };
    let apparent = meta.len();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if apparent >= expected { return (meta.blocks() * 512).min(expected); }
    }
    apparent.min(expected)
}

#[tauri::command]
async fn extract_payload(app: AppHandle, zip_path: String) -> Result<String, String> {
    // Sync commands run on the main (UI) thread in Tauri. Run the long job on a worker thread instead.
    tauri::async_runtime::spawn_blocking(move || extract_payload_blocking(app, zip_path))
        .await
        .map_err(|e| format!("Çıkarma görevi çöktü: {e}"))?
}

fn extract_payload_blocking(app: AppHandle, zip_path: String) -> Result<String, String> {
    let zip = PathBuf::from(&zip_path);
    if !zip.is_file() { return Err("ZIP dosyası bulunamadı.".into()); }
    let stem = zip.file_stem().and_then(|s| s.to_str()).ok_or("ZIP dosya adı okunamadı.")?;
    let parent = zip.parent().unwrap_or(Path::new("."));
    let output_dir = parent.join(stem);
    if output_dir.exists() { return Err(format!("Çıktı klasörü zaten mevcut: {}. Üzerine yazma güvenlik nedeniyle engellendi.", output_dir.display())); }
    let listing = run_output("unzip", &["-Z1", &zip_path]).map_err(|_| "unzip aracı bulunamadı; ZIP içeriği doğrulanamadı.".to_string())?;
    if !listing.status.success() { return Err(format!("ZIP okunamadı: {}", command_text(&listing))); }
    if !String::from_utf8_lossy(&listing.stdout).lines().any(|n| n == "payload.bin") {
        return Err("Seçilen ZIP içinde payload.bin bulunamadı.".into());
    }
    let plan: Vec<(String, u64)> = payload_partitions(&zip).unwrap_or_default();
    let total_bytes: u64 = plan.iter().map(|p| p.1).sum();
    let dumper = find_tool("payload-dumper-go").ok_or_else(|| "payload-dumper-go bulunamadı. Ayarlar → Gerekli araçlar bölümünden kurabilirsin.".to_string())?;
    fs::create_dir(&output_dir).map_err(|e| format!("Çıktı klasörü oluşturulamadı: {e}"))?;
    let mut child = match Command::new(dumper).arg("-o").arg(&output_dir).arg(&zip)
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => { let _ = fs::remove_dir_all(&output_dir); return Err(format!("payload-dumper-go başlatılamadı: {e}")); }
    };
    // Drain stdout and stderr concurrently so a full pipe cannot deadlock the child.
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        thread::spawn(move || pump_lines(stdout, tx));
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        thread::spawn(move || pump_lines(stderr, tx));
    }
    drop(tx);
    let mut seen = std::collections::HashSet::<String>::new();
    let mut done_set = std::collections::HashSet::<String>::new();
    let mut last_payload = String::new();
    let mut last_scan = std::time::Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(std::time::Instant::now);
    let mut last_emit = std::time::Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(std::time::Instant::now);
    let mut recent_lines = Vec::<String>::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(180)) {
            Ok(line) => {
                if !is_progress_noise(&line) {
                    recent_lines.push(line.clone());
                    if recent_lines.len() > 80 { recent_lines.remove(0); }
                    if last_emit.elapsed() >= Duration::from_millis(150) {
                        last_emit = std::time::Instant::now();
                        let _ = app.emit("payload-log", serde_json::json!({"text": line, "kind":"info"}));
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {},
            Err(mpsc::RecvTimeoutError::Disconnected) => { thread::sleep(Duration::from_millis(250)); }
        }
        // Poll the output directory every 500 ms and report REAL progress (bytes written / bytes expected).
        if last_scan.elapsed() >= Duration::from_millis(500) {
            last_scan = std::time::Instant::now();
            if total_bytes > 0 {
                let mut written_total = 0u64;
                let mut items = Vec::with_capacity(plan.len());
                for (name, expected) in &plan {
                    let file = format!("{name}.img");
                    let path = output_dir.join(&file);
                    let exists = path.is_file();
                    let written = if exists { written_bytes(&path, *expected) } else { 0 };
                    written_total += written;
                    let finished = written >= *expected;
                    let state = if finished { "done" } else if exists { "running" } else { "waiting" };
                    if finished && done_set.insert(name.clone()) {
                        let _ = app.emit("payload-log", serde_json::json!({"text": format!("İmaj tamamlandı: {file}"), "kind": "success"}));
                    }
                    items.push(serde_json::json!({"name": file, "expected": expected, "written": written, "state": state}));
                }
                let percent = ((written_total as f64 / total_bytes as f64) * 100.0).floor().min(99.0);
                let payload = serde_json::json!({"percent": percent, "done": done_set.len(), "total": plan.len(), "items": items}).to_string();
                if payload != last_payload {
                    let _ = app.emit("payload-progress", serde_json::from_str::<serde_json::Value>(&payload).unwrap_or_default());
                    last_payload = payload;
                }
            } else if let Ok(entries) = fs::read_dir(&output_dir) {
                // Fallback when the manifest could not be read: only report files as they appear (no percentage).
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("img")).unwrap_or(false) {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if seen.insert(name.to_string()) {
                                let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                                let _ = app.emit("payload-image", serde_json::json!({"name": name, "path": path.to_string_lossy(), "size": size, "count": seen.len()}));
                            }
                        }
                    }
                }
            }
        }
        if let Some(status) = child.try_wait().map_err(|e| format!("Çıkarma süreci kontrol edilemedi: {e}"))? {
            // Read any lines already queued before finalizing.
            while let Ok(line) = rx.try_recv() {
                if !is_progress_noise(&line) { recent_lines.push(line.clone()); let _ = app.emit("payload-log", serde_json::json!({"text":line,"kind":"info"})); }
            }
            if !status.success() {
                let details = recent_lines.join("\n");
                let _ = fs::remove_dir_all(&output_dir);
                let _ = app.emit("payload-finished", serde_json::json!({"ok":false,"count":seen.len()}));
                return Err(format!("Payload çıkarma başarısız ({}): {}", status, details));
            }
            break;
        }
    }
    let _ = app.emit("payload-finished", serde_json::json!({"ok":true,"count":if plan.is_empty(){seen.len()}else{plan.len()}}));
    Ok(output_dir.to_string_lossy().into_owned())
}

// Reads a pipe, splitting on \n and \r (progress bars redraw with \r) and removing ANSI escape
// sequences such as ESC[40A / ESC[J that terminal progress bars emit.
fn pump_lines<R: std::io::Read>(reader: R, tx: mpsc::Sender<String>) {
    let mut buf: Vec<u8> = Vec::new();
    let flush = |buf: &mut Vec<u8>, tx: &mpsc::Sender<String>| {
        if !buf.is_empty() {
            let s = String::from_utf8_lossy(buf).trim().to_string();
            if !s.is_empty() { let _ = tx.send(s); }
            buf.clear();
        }
    };
    let mut it = BufReader::new(reader).bytes().flatten().peekable();
    while let Some(b) = it.next() {
        match b {
            0x1b => match it.peek().copied() {
                Some(b'[') => { it.next(); while let Some(c) = it.next() { if (0x40..=0x7e).contains(&c) { break; } } }
                Some(_) => { it.next(); }
                None => {}
            },
            b'\n' | b'\r' => flush(&mut buf, &tx),
            _ => buf.push(b),
        }
    }
    flush(&mut buf, &tx);
}

/// True for redrawn terminal progress-bar lines; the UI shows real progress from file sizes instead.
fn is_progress_noise(line: &str) -> bool {
    let t = line.trim();
    t.is_empty()
        || t.chars().all(|c| !c.is_alphanumeric())
        || t.contains("[=") || t.contains("[>") || t.contains("=>") || t.contains("=]") || t.contains('█') || t.contains("p/s")
}

fn is_bootloader_image(name: &str) -> bool {
    matches!(name, "boot.img" | "dtbo.img" | "init_boot.img" | "modem.img" |
        "recovery.img" | "vbmeta.img" | "vbmeta_system.img" | "vbmeta_vendor.img" | "vendor_boot.img")
}

#[tauri::command]
async fn build_flash_plan(path: String, names: Vec<String>, serial: String) -> Result<Vec<FlashStep>, String> {
    tauri::async_runtime::spawn_blocking(move || build_flash_plan_blocking(path, names, serial)).await.map_err(|e| e.to_string())?
}

fn build_flash_plan_blocking(path: String, names: Vec<String>, serial: String) -> Result<Vec<FlashStep>, String> {
    if serial.trim().is_empty() { return Err("Hedef cihazın seri numarası boş.".into()); }
    let root = PathBuf::from(&path);
    if !root.is_dir() { return Err("ROM klasörü mevcut değil.".into()); }
    // Confirm the serial is currently visible through either ADB or Fastboot.
    let found = list_devices_blocking()?.into_iter().find(|d| d.serial == serial);
    let Some(found) = found else { return Err(format!("Hedef cihaz {serial} artık bağlı görünmüyor. Flashlama iptal edildi.")); };
    if found.mode != "Bootloader Fastboot" {
        return Err(format!("Flashlama planı Bootloader Fastboot modunda başlatılmalı. Algılanan mod: {}. Önce Device Control üzerinden Bootloader'a geç.", found.mode));
    }
    let mut boot = Vec::new();
    let mut rest = Vec::new();
    for name in names {
        if name.contains('/') || name.contains('\\') || !name.ends_with(".img") { return Err(format!("Geçersiz imaj adı: {name}")); }
        let image_path = root.join(&name);
        if !image_path.is_file() { return Err(format!("Seçilen imaj bulunamadı: {}", image_path.display())); }
        let partition = name.trim_end_matches(".img").to_string();
        if is_bootloader_image(&name) { boot.push((name, image_path, partition)); }
        else { rest.push((name, image_path, partition)); }
    }
    let mut plan = Vec::new();
    for (name, image_path, partition) in boot {
        for slot in ["a", "b"] {
            plan.push(FlashStep { mode: "Bootloader Fastboot".into(), image: name.clone(), partition: format!("{partition}_{slot}"), path: image_path.to_string_lossy().into_owned(), action: "flash".into(), critical: true });
        }
    }
    if !rest.is_empty() {
        plan.push(FlashStep { mode: "Bootloader".into(), image: "—".into(), partition: "fastbootd".into(), path: String::new(), action: "reboot-fastbootd".into(), critical: true });
    }
    for (name, image_path, partition) in rest {
        plan.push(FlashStep { mode: "Fastbootd".into(), image: name, partition, path: image_path.to_string_lossy().into_owned(), action: "flash-slot-all".into(), critical: true });
    }
    if plan.is_empty() { return Err("Flash planı boş.".into()); }
    Ok(plan)
}

#[tauri::command]
async fn flash_step(step: FlashStep, serial: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || flash_step_blocking(step, serial)).await.map_err(|e| e.to_string())?
}

fn flash_step_blocking(step: FlashStep, serial: String) -> Result<String, String> {
    if serial.trim().is_empty() { return Err("Hedef seri numarası boş.".into()); }
    // Recheck the target before every destructive operation to avoid flashing a replacement device.
    if step.action != "reboot-fastbootd" {
        let current = list_devices_blocking()?.into_iter().find(|d| d.serial == serial)
            .ok_or_else(|| format!("Hedef cihaz {serial} bağlantısı kayboldu. İşlem durduruldu."))?;
        if current.mode != step.mode {
            return Err(format!("Yanlış cihaz modu: {} gerekli, algılanan {}. İşlem durduruldu.", step.mode, current.mode));
        }
    }
    let args: Vec<String> = match step.action.as_str() {
        "reboot-fastbootd" => vec!["-s".into(), serial.clone(), "reboot".into(), "fastboot".into()],
        "flash" => {
            if !Path::new(&step.path).is_file() { return Err(format!("İmaj dosyası bulunamadı: {}", step.path)); }
            vec!["-s".into(), serial.clone(), "flash".into(), step.partition.clone(), step.path.clone()]
        }
        "flash-slot-all" => {
            if !Path::new(&step.path).is_file() { return Err(format!("İmaj dosyası bulunamadı: {}", step.path)); }
            vec!["-s".into(), serial.clone(), "flash".into(), "--slot=all".into(), step.partition.clone(), step.path.clone()]
        }
        _ => return Err(format!("Desteklenmeyen flash adımı: {}", step.action)),
    };
    let (program, args_ref): (&str, Vec<&str>) = if step.action == "reboot-fastbootd" {
        ("fastboot", args.iter().map(String::as_str).collect())
    } else {
        ("fastboot", args.iter().map(String::as_str).collect())
    };
    let out = run_output(program, &args_ref)?;
    let message = command_text(&out);
    if !out.status.success() {
        return Err(format!("{} → {} başarısız: {}", step.image, step.partition, if message.is_empty() { "fastboot hata koduyla çıktı".into() } else { message }));
    }
    if step.action == "reboot-fastbootd" {
        // The USB device may disappear briefly while switching Fastboot modes.
        let mut returned = false;
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(750));
            if list_devices_blocking()?.iter().any(|d| d.serial == serial && d.mode == "Fastbootd") {
                returned = true;
                break;
            }
        }
        if !returned { return Err(format!("{} fastbootd moduna geçtikten sonra yeniden algılanamadı. Kalan işlemler durduruldu.", serial)); }
    }
    Ok(if message.is_empty() { format!("OK: {} → {}", step.image, step.partition) } else { message })
}

#[tauri::command]
async fn device_action(action: String, serial: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || device_action_blocking(action, serial)).await.map_err(|e| e.to_string())?
}

fn device_action_blocking(action: String, serial: String) -> Result<String, String> {
    let current = list_devices_blocking()?.into_iter().find(|d| d.serial == serial)
        .ok_or_else(|| "Hedef cihaz bağlı değil.".to_string())?;
    // The tool is chosen from the device's *detected* mode: ADB for Android, Fastboot for bootloader/fastbootd.
    let program = if current.mode == "Bootloader Fastboot" || current.mode == "Fastbootd" { "fastboot" } else { "adb" };
    let target = match action.as_str() {
        "system" => None,
        "bootloader" => Some("bootloader"),
        "fastbootd" => Some("fastboot"),
        "recovery" => Some("recovery"),
        _ => return Err("Bilinmeyen cihaz işlemi.".into()),
    };
    let mut args: Vec<&str> = vec!["-s", serial.as_str(), "reboot"];
    if let Some(t) = target { args.push(t); }
    let out = run_output(program, &args)?;
    let message = command_text(&out);
    if !out.status.success() { return Err(format!("{program} komutu başarısız: {message}")); }
    Ok(if message.is_empty() { format!("{action} komutu gönderildi ({program}); mod geçişi yeniden doğrulanmalı.") } else { message })
}

#[tauri::command]
async fn wipe_data(serial: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || wipe_data_blocking(serial)).await.map_err(|e| e.to_string())?
}

/// Formats user data with `fastboot -w` (erase userdata + metadata). Falls back to explicit erases if `-w` fails.
fn wipe_data_blocking(serial: String) -> Result<String, String> {
    if serial.trim().is_empty() { return Err("Hedef seri numarası boş.".into()); }
    let current = list_devices_blocking()?.into_iter().find(|d| d.serial == serial)
        .ok_or_else(|| format!("Hedef cihaz {serial} bağlı görünmüyor. Format işlemi iptal edildi."))?;
    if current.mode != "Bootloader Fastboot" && current.mode != "Fastbootd" {
        return Err(format!("Format işlemi için cihaz Fastboot modunda olmalı. Algılanan mod: {}.", current.mode));
    }
    let out = run_output("fastboot", &["-s", serial.as_str(), "-w"])?;
    let first = command_text(&out);
    if out.status.success() {
        return Ok(if first.is_empty() { "fastboot -w tamamlandı: userdata formatlandı.".into() } else { first });
    }
    // Fallback: explicit erase of userdata (required) and metadata (best effort, not present on every device).
    let erase_user = run_output("fastboot", &["-s", serial.as_str(), "erase", "userdata"])?;
    let user_text = command_text(&erase_user);
    if !erase_user.status.success() {
        return Err(format!("Format başarısız.\nfastboot -w: {first}\nerase userdata: {user_text}"));
    }
    let erase_meta = run_output("fastboot", &["-s", serial.as_str(), "erase", "metadata"])?;
    let meta_note = if erase_meta.status.success() { "metadata de silindi" } else { "metadata silinemedi (bu cihazda olmayabilir)" };
    Ok(format!("fastboot -w başarısız oldu; yedek yöntemle userdata silindi, {meta_note}.\n{user_text}"))
}

// ================= Tool detection & one-click installer =================
fn home_dir() -> Option<PathBuf> { std::env::var_os("HOME").map(PathBuf::from) }

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// PATH lookup plus the places this app installs to (a desktop-launched app often lacks ~/.local/bin in PATH).
fn find_tool(name: &str) -> Option<PathBuf> {
    if let Ok(p) = which::which(name) { return Some(p); }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(h) = home_dir() {
        dirs.push(h.join(".local/bin"));
        dirs.push(h.join(".local/share/linux-flasher/platform-tools"));
    }
    for d in ["/usr/local/bin", "/usr/bin", "/bin", "/usr/local/sbin", "/usr/sbin", "/sbin"] { dirs.push(PathBuf::from(d)); }
    dirs.into_iter().map(|d| d.join(name)).find(|p| is_executable(p))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolStatus { id: String, name: String, installed: bool, path: String, version: String }

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolsReport { package_manager: String, elevation: String, arch: String, tools: Vec<ToolStatus> }

fn tool_log(app: &AppHandle, tool: &str, text: &str, kind: &str) {
    let _ = app.emit("tool-log", serde_json::json!({"tool": tool, "text": text, "kind": kind}));
}

fn first_line(program: &Path, args: &[&str]) -> String {
    Command::new(program).args(args).output().ok().map(|o| {
        let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        text.lines().next().unwrap_or("").trim().to_string()
    }).unwrap_or_default()
}

fn detect_pm() -> Option<&'static str> {
    ["apt-get", "dnf", "pacman", "zypper", "apk", "xbps-install", "yum"].into_iter().find(|pm| find_tool(pm).is_some())
}

fn is_root() -> bool {
    fs::read_to_string("/proc/self/status").ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).and_then(|l| l.split_whitespace().nth(1).map(|u| u == "0")))
        .unwrap_or(false)
}

/// Fixed (never user-supplied) install scripts per package manager. One script = one password prompt.
fn pm_script(pm: &str, tool: &str) -> Option<String> {
    let apt = "export DEBIAN_FRONTEND=noninteractive; apt-get update; ";
    Some(match (pm, tool) {
        ("apt-get", "unzip") => format!("{apt}apt-get install -y unzip"),
        ("apt-get", "platform-tools") => format!("{apt}apt-get install -y adb fastboot || apt-get install -y android-tools-adb android-tools-fastboot"),
        ("dnf" | "yum", "unzip") => format!("{pm} install -y unzip"),
        ("dnf" | "yum", "platform-tools") => format!("{pm} install -y android-tools"),
        ("pacman", "unzip") => "pacman -S --needed --noconfirm unzip".into(),
        ("pacman", "platform-tools") => "pacman -S --needed --noconfirm android-tools".into(),
        ("zypper", "unzip") => "zypper --non-interactive install unzip".into(),
        ("zypper", "platform-tools") => "zypper --non-interactive install android-tools".into(),
        ("apk", "unzip") => "apk add --no-cache unzip".into(),
        ("apk", "platform-tools") => "apk add --no-cache android-tools".into(),
        ("xbps-install", "unzip") => "xbps-install -Sy unzip".into(),
        ("xbps-install", "platform-tools") => "xbps-install -Sy android-tools".into(),
        _ => return None,
    })
}

fn elevated(script: &str) -> Result<Command, String> {
    let sh = find_tool("sh").ok_or_else(|| "sh bulunamadı.".to_string())?;
    if is_root() {
        let mut c = Command::new(sh);
        c.arg("-c").arg(script);
        return Ok(c);
    }
    if let Some(pk) = find_tool("pkexec") {
        let mut c = Command::new(pk);
        c.arg(sh).arg("-c").arg(script);
        return Ok(c);
    }
    Err(format!("Yönetici yetkisi için pkexec (polkit) gerekli ama bulunamadı. Terminalde root olarak şunu çalıştır:\n{script}"))
}

fn tool_present(id: &str) -> Option<PathBuf> {
    match id {
        "platform-tools" => { let adb = find_tool("adb")?; find_tool("fastboot")?; Some(adb) }
        "unzip" => find_tool("unzip"),
        "payload-dumper-go" => find_tool("payload-dumper-go"),
        _ => None,
    }
}

#[tauri::command]
async fn check_tools() -> Result<ToolsReport, String> {
    tauri::async_runtime::spawn_blocking(check_tools_blocking).await.map_err(|e| e.to_string())?
}

fn check_tools_blocking() -> Result<ToolsReport, String> {
    let defs = [("platform-tools", "Android platform-tools (adb + fastboot)"), ("unzip", "unzip"), ("payload-dumper-go", "payload-dumper-go")];
    let tools = defs.iter().map(|(id, name)| {
        let path = tool_present(id);
        let version = match (*id, &path) {
            ("platform-tools", Some(p)) => first_line(p, &["version"]),
            ("unzip", Some(p)) => first_line(p, &["-v"]),
            _ => String::new(),
        };
        ToolStatus { id: id.to_string(), name: name.to_string(), installed: path.is_some(), path: path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), version }
    }).collect();
    let elevation = if is_root() { "root" } else if find_tool("pkexec").is_some() { "pkexec" } else { "none" };
    Ok(ToolsReport { package_manager: detect_pm().unwrap_or("").to_string(), elevation: elevation.into(), arch: std::env::consts::ARCH.into(), tools })
}

/// Runs a command, streaming its output to the UI. Returns (exit code, last output lines).
fn run_streaming(app: &AppHandle, tool: &str, mut cmd: Command) -> Result<(i32, Vec<String>), String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Komut başlatılamadı: {e}"))?;
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(o) = child.stdout.take() { let tx = tx.clone(); thread::spawn(move || pump_lines(o, tx)); }
    if let Some(e) = child.stderr.take() { let tx = tx.clone(); thread::spawn(move || pump_lines(e, tx)); }
    drop(tx);
    let mut tail: Vec<String> = Vec::new();
    let mut last = std::time::Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(std::time::Instant::now);
    for line in rx {
        if line.trim().is_empty() { continue; }
        tail.push(line.clone());
        if tail.len() > 40 { tail.remove(0); }
        if last.elapsed() >= Duration::from_millis(100) { last = std::time::Instant::now(); tool_log(app, tool, &line, "info"); }
    }
    let status = child.wait().map_err(|e| format!("Süreç beklenemedi: {e}"))?;
    Ok((status.code().unwrap_or(-1), tail))
}

fn install_via_pm(app: &AppHandle, tool: &str) -> Result<(), String> {
    let pm = detect_pm().ok_or_else(|| "Desteklenen bir paket yöneticisi bulunamadı (apt, dnf, yum, pacman, zypper, apk, xbps).".to_string())?;
    let script = pm_script(pm, tool).ok_or_else(|| format!("{pm} için {tool} paketi tanımlı değil."))?;
    tool_log(app, tool, &format!("Paket yöneticisi: {pm}. Yönetici parolası istenebilir…"), "info");
    let (code, tail) = run_streaming(app, tool, elevated(&script)?)?;
    match code {
        0 => Ok(()),
        126 | 127 if !is_root() => Err("Yetkilendirme iptal edildi veya başarısız oldu.".into()),
        _ => Err(format!("{pm} kurulumu başarısız (çıkış kodu {code}):\n{}", tail.join("\n"))),
    }
}

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let run = if let Some(curl) = find_tool("curl") {
        Command::new(curl).args(["-fsSL", "--retry", "3", "--connect-timeout", "20", "-H", "Accept: application/vnd.github+json", "-H", "User-Agent: linux-flasher", url]).output()
    } else if let Some(wget) = find_tool("wget") {
        Command::new(wget).args(["-qO-", "--header=Accept: application/vnd.github+json", "--user-agent=linux-flasher", url]).output()
    } else {
        return Err("İndirme için curl veya wget gerekli.".into());
    };
    let out = run.map_err(|e| format!("İndirme başlatılamadı: {e}"))?;
    if !out.status.success() { return Err(format!("İndirme başarısız ({url}): {}", String::from_utf8_lossy(&out.stderr).trim())); }
    Ok(out.stdout)
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let dest_s = dest.to_string_lossy().into_owned();
    let run = if let Some(curl) = find_tool("curl") {
        Command::new(curl).args(["-fL", "--retry", "3", "--connect-timeout", "20", "-sS", "-o", dest_s.as_str(), url]).status()
    } else if let Some(wget) = find_tool("wget") {
        Command::new(wget).args(["-q", "-O", dest_s.as_str(), url]).status()
    } else {
        return Err("İndirme için curl veya wget gerekli.".into());
    };
    let status = run.map_err(|e| format!("İndirme başlatılamadı: {e}"))?;
    if !status.success() { return Err(format!("İndirme başarısız: {url}")); }
    Ok(())
}

fn extract_zip(zip: &Path, dest: &Path) -> Result<(), String> {
    let (z, d) = (zip.to_string_lossy().into_owned(), dest.to_string_lossy().into_owned());
    let run = if let Some(u) = find_tool("unzip") {
        Command::new(u).args(["-oq", z.as_str(), "-d", d.as_str()]).status()
    } else if let Some(b) = find_tool("bsdtar") {
        Command::new(b).args(["-xf", z.as_str(), "-C", d.as_str()]).status()
    } else if let Some(p) = find_tool("python3") {
        Command::new(p).args(["-m", "zipfile", "-e", z.as_str(), d.as_str()]).status()
    } else {
        return Err("ZIP açmak için unzip, bsdtar veya python3 gerekli.".into());
    };
    let status = run.map_err(|e| format!("ZIP açılamadı: {e}"))?;
    if !status.success() { return Err("ZIP açma başarısız.".into()); }
    Ok(())
}

fn temp_work_dir(tag: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("linux-flasher-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("Geçici klasör oluşturulamadı: {e}"))?;
    Ok(dir)
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() { if let Some(f) = find_file(&p, name) { return Some(f); } }
        else if p.file_name().and_then(|n| n.to_str()) == Some(name) { return Some(p); }
    }
    None
}

fn is_elf(p: &Path) -> bool {
    let mut magic = [0u8; 4];
    fs::File::open(p).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == b"\x7fELF"
}

fn make_executable(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o755)).map_err(|e| format!("İzin ayarlanamadı: {e}"))
}

/// Fallback without root: Google's official platform-tools (x86_64 only), linked into ~/.local/bin.
fn install_platform_tools_download(app: &AppHandle) -> Result<String, String> {
    if std::env::consts::ARCH != "x86_64" {
        return Err(format!("Google platform-tools yalnızca x86_64 için sunuluyor (bu sistem: {}). Dağıtımının paket yöneticisiyle kur.", std::env::consts::ARCH));
    }
    let home = home_dir().ok_or_else(|| "HOME bulunamadı.".to_string())?;
    let base = home.join(".local/share/linux-flasher");
    let bin = home.join(".local/bin");
    fs::create_dir_all(&base).and_then(|_| fs::create_dir_all(&bin)).map_err(|e| format!("Klasör oluşturulamadı: {e}"))?;
    let tmp = temp_work_dir("pt")?;
    let zip = tmp.join("platform-tools.zip");
    tool_log(app, "platform-tools", "Google'dan platform-tools indiriliyor…", "info");
    download("https://dl.google.com/android/repository/platform-tools-latest-linux.zip", &zip)?;
    let dest = base.join("platform-tools");
    let _ = fs::remove_dir_all(&dest);
    tool_log(app, "platform-tools", "Arşiv açılıyor…", "info");
    extract_zip(&zip, &base)?;
    let _ = fs::remove_dir_all(&tmp);
    for exe in ["adb", "fastboot"] {
        let src = dest.join(exe);
        if !src.is_file() { return Err(format!("Arşivde {exe} bulunamadı.")); }
        make_executable(&src)?;
        let link = bin.join(exe);
        if let Ok(m) = fs::symlink_metadata(&link) {
            if m.file_type().is_symlink() { let _ = fs::remove_file(&link); }
            else { tool_log(app, "platform-tools", &format!("{} zaten var, bağlantı oluşturulmadı.", link.display()), "info"); continue; }
        }
        std::os::unix::fs::symlink(&src, &link).map_err(|e| format!("Bağlantı oluşturulamadı: {e}"))?;
    }
    Ok(format!("Android platform-tools kuruldu: {}", dest.display()))
}

fn install_payload_dumper(app: &AppHandle) -> Result<String, String> {
    let arch = match std::env::consts::ARCH { "x86_64" => "amd64", "aarch64" => "arm64", other => return Err(format!("Desteklenmeyen mimari: {other}")) };
    tool_log(app, "payload-dumper-go", "En güncel sürüm GitHub'dan sorgulanıyor…", "info");
    let body = http_get("https://api.github.com/repos/ssut/payload-dumper-go/releases/latest")?;
    let release: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("GitHub yanıtı okunamadı: {e}"))?;
    let tag = release["tag_name"].as_str().unwrap_or("?").to_string();
    let asset = release["assets"].as_array().and_then(|a| a.iter().find(|x| {
        let n = x["name"].as_str().unwrap_or("").to_lowercase();
        n.contains("linux") && n.contains(arch) && (n.ends_with(".tar.gz") || n.ends_with(".tgz") || n.ends_with(".zip"))
    })).ok_or_else(|| format!("{tag} sürümünde linux/{arch} paketi bulunamadı."))?;
    let name = asset["name"].as_str().unwrap_or("payload-dumper-go.tar.gz").to_string();
    let url = asset["browser_download_url"].as_str().ok_or_else(|| "İndirme adresi bulunamadı.".to_string())?.to_string();
    if !url.starts_with("https://github.com/ssut/payload-dumper-go/") { return Err(format!("Beklenmeyen indirme adresi: {url}")); }
    let tmp = temp_work_dir("pdg")?;
    let archive = tmp.join(&name);
    tool_log(app, "payload-dumper-go", &format!("{name} indiriliyor ({tag})…"), "info");
    download(&url, &archive)?;
    let out_dir = tmp.join("out");
    fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    if name.to_lowercase().ends_with(".zip") { extract_zip(&archive, &out_dir)?; }
    else {
        let tar = find_tool("tar").ok_or_else(|| "tar bulunamadı.".to_string())?;
        let ok = Command::new(tar).arg("-xzf").arg(&archive).arg("-C").arg(&out_dir).status().map(|s| s.success()).unwrap_or(false);
        if !ok { return Err("Arşiv açılamadı (tar).".into()); }
    }
    let binary = find_file(&out_dir, "payload-dumper-go").ok_or_else(|| "Arşivde payload-dumper-go bulunamadı.".to_string())?;
    if !is_elf(&binary) { return Err("İndirilen dosya geçerli bir Linux programı değil.".into()); }
    let bin_dir = home_dir().ok_or_else(|| "HOME bulunamadı.".to_string())?.join(".local/bin");
    fs::create_dir_all(&bin_dir).map_err(|e| format!("Klasör oluşturulamadı: {e}"))?;
    let dest = bin_dir.join("payload-dumper-go");
    let _ = fs::remove_file(&dest);
    fs::copy(&binary, &dest).map_err(|e| format!("Kopyalanamadı: {e}"))?;
    make_executable(&dest)?;
    let _ = fs::remove_dir_all(&tmp);
    Ok(format!("payload-dumper-go {tag} kuruldu: {}", dest.display()))
}

#[tauri::command]
async fn install_tool(app: AppHandle, id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || install_tool_blocking(app, id)).await.map_err(|e| e.to_string())?
}

fn install_tool_blocking(app: AppHandle, id: String) -> Result<String, String> {
    let result = match id.as_str() {
        "unzip" => install_via_pm(&app, "unzip").map(|_| "unzip kuruldu.".to_string()),
        "platform-tools" => match install_via_pm(&app, "platform-tools") {
            Ok(()) if tool_present("platform-tools").is_some() => Ok("Android platform-tools kuruldu.".to_string()),
            first => {
                let why = match first { Err(e) => e, Ok(()) => "Paket kuruldu ama adb/fastboot bulunamadı.".into() };
                tool_log(&app, "platform-tools", &format!("Paket yöneticisi yöntemi olmadı, Google'dan indirme deneniyor. ({})", why.lines().next().unwrap_or("")), "info");
                install_platform_tools_download(&app).map_err(|e| format!("{why}\n\nYedek yöntem de başarısız: {e}"))
            }
        },
        "payload-dumper-go" => install_payload_dumper(&app),
        other => Err(format!("Bilinmeyen araç: {other}")),
    };
    if let Err(e) = &result { tool_log(&app, &id, e.lines().next().unwrap_or("Hata"), "error"); }
    else { tool_log(&app, &id, "Tamamlandı.", "success"); }
    result
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_devices, choose_rom_folder, choose_zip, scan_images,
            extract_payload, build_flash_plan, flash_step, device_action, wipe_data, check_tools, install_tool,
            backup::root_check, backup::list_packages, backup::backup_apps, backup::restore_apps, backup::list_backups,
            ksu::ksu_releases, ksu::ksu_scan, ksu::ksu_detect_kmi, ksu::ksu_patch, ksu::ksu_flash, ksu::ksu_install_manager,
            backup::app_icons, backup::default_backup_dir, backup::choose_dir, backup::backup_cancel
        ])
        .run(tauri::generate_context!())
        .expect("error while running Linux Flasher");
}
