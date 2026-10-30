//! KSU patching on the PC (no manager app needed).
//!
//! KernelSU-family projects publish a `ksud` binary for Linux. `ksud boot-patch` (LKM mode) unpacks a
//! boot / init_boot image, injects `ksuinit` + the matching `<kmi>_kernelsu.ko`, repacks and writes a
//! patched image. This module downloads the right `ksud` (+ `.ko`) from a GitHub release, runs it and
//! can flash the result with fastboot. Only GKI devices (a published KMI such as `android14-6.1`) work.
use super::{command_text, download, find_tool, home_dir, http_get, list_devices_blocking, make_executable, pump_lines, run_output};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime},
};
use tauri::{AppHandle, Emitter};

fn ok_repo(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && p.len() <= 100 && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
}
fn ok_tag(s: &str) -> bool { !s.is_empty() && s.len() <= 80 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+')) }
fn ok_asset_name(s: &str) -> bool { !s.is_empty() && s.len() <= 120 && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+')) }
fn ok_kmi(s: &str) -> bool {
    match s.split_once('-') {
        Some((a, k)) => a.len() > 7 && a.starts_with("android") && a[7..].chars().all(|c| c.is_ascii_digit()) && !k.is_empty() && k.chars().all(|c| c.is_ascii_digit() || c == '.'),
        None => false,
    }
}
fn ok_serial(s: &str) -> bool { !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-')) }

fn emit_log(app: &AppHandle, text: &str, kind: &str) { let _ = app.emit("ksu-log", json!({"text": text, "kind": kind})); }
fn emit_step(app: &AppHandle, step: &str, state: &str, text: &str) { let _ = app.emit("ksu-step", json!({"step": step, "state": state, "text": text})); }

#[derive(Debug, Clone)]
struct Asset { name: String, url: String, size: u64, digest: Option<String> }

fn assets_of(rel: &Value) -> Vec<Asset> {
    rel["assets"].as_array().map(|a| a.iter().filter_map(|x| Some(Asset {
        name: x["name"].as_str()?.to_string(), url: x["browser_download_url"].as_str()?.to_string(), size: x["size"].as_u64().unwrap_or(0),
        digest: x["digest"].as_str().map(|s| s.to_string()),
    })).collect()).unwrap_or_default()
}

fn kmi_of_asset(name: &str) -> Option<String> {
    let k = name.strip_suffix("_kernelsu.ko")?;
    ok_kmi(k).then(|| k.to_string())
}

fn host_arch_token() -> Option<&'static str> { match std::env::consts::ARCH { "x86_64" => Some("x86_64"), "aarch64" => Some("aarch64"), _ => None } }

fn pick_ksud(assets: &[Asset]) -> Option<&Asset> {
    let arch = host_arch_token()?;
    assets.iter().filter(|a| {
        let n = a.name.to_lowercase();
        n.starts_with("ksud") && n.contains(arch) && n.contains("linux") && !n.ends_with(".sha256") && !n.ends_with(".zip") && !n.ends_with(".txt")
    }).max_by_key(|a| a.name.contains("musl"))
}

/// The manager APK of a release (the plain build by default, the "spoofed" one when requested).
fn pick_manager(assets: &[Asset], spoofed: bool) -> Option<&Asset> {
    let apks: Vec<&Asset> = assets.iter().filter(|a| { let n = a.name.to_lowercase(); n.ends_with(".apk") && !n.contains("debug") }).collect();
    let mut chosen: Option<&Asset> = None;
    for a in &apks {
        if a.name.to_lowercase().contains("spoof") == spoofed { chosen = Some(*a); break; }
    }
    chosen.or_else(|| apks.first().copied())
}

fn pick_magiskboot(assets: &[Asset]) -> Option<&Asset> {
    let arch = host_arch_token()?;
    assets.iter().find(|a| { let n = a.name.to_lowercase(); n.contains("magiskboot") && n.contains("linux") && (n.contains(arch) || !n.contains("aarch64")) && !n.ends_with(".sha256") })
}

fn github_json(url: &str) -> Result<Value, String> {
    let body = http_get(url)?;
    serde_json::from_slice(&body).map_err(|e| format!("GitHub yanıtı okunamadı: {e}"))
}

// ---------------------------------------------------------------- releases
/// Latest releases of a KernelSU-family repo with the KMIs they ship an LKM for.
#[tauri::command]
pub async fn ksu_releases(repo: String) -> Result<Vec<Value>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !ok_repo(&repo) { return Err("Geçersiz depo adı (sahip/depo).".to_string()); }
        let list = github_json(&format!("https://api.github.com/repos/{repo}/releases?per_page=8"))?;
        let arr = list.as_array().ok_or_else(|| "Sürüm listesi alınamadı.".to_string())?;
        let mut out = Vec::new();
        for rel in arr.iter().filter(|r| r["draft"] != true) {
            let assets = assets_of(rel);
            let mut kmis: Vec<String> = assets.iter().filter_map(|a| kmi_of_asset(&a.name)).collect();
            kmis.sort(); kmis.dedup();
            out.push(json!({
                "tag": rel["tag_name"], "name": rel["name"], "prerelease": rel["prerelease"], "publishedAt": rel["published_at"],
                "kmis": kmis, "ksud": pick_ksud(&assets).is_some(), "magiskboot": pick_magiskboot(&assets).is_some(),
                "apk": pick_manager(&assets, false).is_some(), "spoofedApk": assets.iter().any(|a| { let n = a.name.to_lowercase(); n.ends_with(".apk") && n.contains("spoof") }),
            }));
        }
        Ok(out)
    }).await.map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- folder scan
#[tauri::command]
pub async fn ksu_scan(folder: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = PathBuf::from(&folder);
        if !dir.is_dir() { return Err("Klasör bulunamadı.".to_string()); }
        let mut images = Vec::new();
        for name in ["init_boot.img", "boot.img", "vendor_boot.img", "vbmeta.img"] {
            let p = dir.join(name);
            if let Ok(m) = fs::metadata(&p) { if m.is_file() { images.push(json!({"name": name, "size": m.len(), "path": p.to_string_lossy()})); } }
        }
        let has = |n: &str| images.iter().any(|i| i["name"] == n);
        let recommended = if has("init_boot.img") { Some("init_boot") } else if has("boot.img") { Some("boot") } else { None };
        Ok(json!({"folder": folder, "images": images, "recommended": recommended}))
    }).await.map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- KMI detection
/// "6.1.75-android14-11-g…" / "Linux version 6.1.75-android14-11…" -> "android14-6.1"
fn parse_kmi(s: &str) -> Option<String> {
    let i = s.find("-android")?;
    let av: String = s[i + 1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if !av.starts_with("android") { return None; }
    let rev: String = s[..i].chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let ver: String = rev.chars().rev().collect();
    let mut p = ver.split('.');
    let (a, b) = (p.next()?, p.next()?);
    let kmi = format!("{av}-{a}.{b}");
    ok_kmi(&kmi).then_some(kmi)
}

#[tauri::command]
pub async fn ksu_detect_kmi(serial: Option<String>, folder: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // 1) the running kernel of a connected Android device
        if let Some(s) = serial.as_deref().filter(|s| ok_serial(s)) {
            let on_adb = list_devices_blocking().ok().and_then(|d| d.into_iter().find(|x| x.serial == s)).map(|d| d.mode.starts_with("ADB")).unwrap_or(false);
            if on_adb {
                if let Ok(out) = run_output("adb", &["-s", s, "shell", "uname", "-r"]) {
                    let text = command_text(&out);
                    if let Some(k) = parse_kmi(&text) { return Ok(json!({"kmi": k, "source": "device", "kernel": text})); }
                }
            }
        }
        // 2) an uncompressed kernel inside boot.img
        let boot = PathBuf::from(&folder).join("boot.img");
        if let Ok(bytes) = fs::read(&boot) {
            let needle = b"Linux version ";
            if let Some(pos) = bytes.windows(needle.len()).position(|w| w == needle) {
                let end = (pos + 160).min(bytes.len());
                let text = String::from_utf8_lossy(&bytes[pos..end]).into_owned();
                let line = text.lines().next().unwrap_or("").to_string();
                if let Some(k) = parse_kmi(&line) { return Ok(json!({"kmi": k, "source": "boot.img", "kernel": line})); }
            }
        }
        Ok(json!({"kmi": null, "source": "none", "kernel": ""}))
    }).await.map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- patching
fn cache_dir(repo: &str, tag: &str) -> Result<PathBuf, String> {
    let d = home_dir().ok_or_else(|| "HOME bulunamadı.".to_string())?.join(".cache/linux-flasher/ksu").join(repo.replace('/', "__")).join(tag);
    fs::create_dir_all(&d).map_err(|e| format!("Önbellek klasörü oluşturulamadı: {e}"))?;
    Ok(d)
}

fn verify_digest(path: &Path, digest: &Option<String>) -> Result<(), String> {
    let Some(want) = digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) else { return Ok(()) };
    let Some(tool) = find_tool("sha256sum") else { return Ok(()) };
    let out = Command::new(tool).arg(path).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let got = text.split_whitespace().next().unwrap_or("");
    if got.eq_ignore_ascii_case(want) { Ok(()) } else { Err("İndirilen dosyanın SHA-256 değeri uyuşmuyor; dosya silindi.".into()) }
}

fn ensure_asset(app: &AppHandle, repo: &str, dir: &Path, a: &Asset) -> Result<PathBuf, String> {
    if !ok_asset_name(&a.name) { return Err("Geçersiz dosya adı.".into()); }
    if !a.url.starts_with(&format!("https://github.com/{repo}/releases/download/")) { return Err(format!("Beklenmeyen indirme adresi: {}", a.url)); }
    let p = dir.join(&a.name);
    if fs::metadata(&p).map(|m| m.len() == a.size && a.size > 0).unwrap_or(false) && verify_digest(&p, &a.digest).is_ok() { return Ok(p); }
    emit_log(app, &format!("{} indiriliyor…", a.name), "info");
    download(&a.url, &p)?;
    if let Err(e) = verify_digest(&p, &a.digest) { let _ = fs::remove_file(&p); return Err(e); }
    Ok(p)
}

/// Runs a command, streaming its output to the UI. Returns (success, last lines).
fn run_logged(app: &AppHandle, mut cmd: Command) -> Result<(bool, Vec<String>), String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Komut başlatılamadı: {e}"))?;
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(o) = child.stdout.take() { let tx = tx.clone(); thread::spawn(move || pump_lines(o, tx)); }
    if let Some(e) = child.stderr.take() { let tx = tx.clone(); thread::spawn(move || pump_lines(e, tx)); }
    drop(tx);
    let mut tail: Vec<String> = Vec::new();
    for line in rx {
        emit_log(app, &line, "info");
        tail.push(line);
        if tail.len() > 60 { tail.remove(0); }
    }
    let st = child.wait().map_err(|e| format!("Süreç beklenemedi: {e}"))?;
    Ok((st.success(), tail))
}

#[tauri::command]
pub async fn ksu_patch(app: AppHandle, repo: String, tag: String, folder: String, partition: String, kmi: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || ksu_patch_blocking(&app, &repo, &tag, &folder, &partition, &kmi)).await.map_err(|e| e.to_string())?
}

fn ksu_patch_blocking(app: &AppHandle, repo: &str, tag: &str, folder: &str, partition: &str, kmi: &str) -> Result<Value, String> {
    if !ok_repo(repo) || !ok_tag(tag) { return Err("Geçersiz depo veya sürüm.".into()); }
    if !ok_kmi(kmi) { return Err(format!("Geçersiz KMI: {kmi}")); }
    if partition != "init_boot" && partition != "boot" { return Err("Bölüm init_boot veya boot olmalı.".into()); }
    let img = PathBuf::from(folder).join(format!("{partition}.img"));
    if !img.is_file() { return Err(format!("{partition}.img klasörde bulunamadı.")); }

    // 1) download ksud (+ matching LKM) from the release
    emit_step(app, "download", "running", repo);
    let rel = github_json(&format!("https://api.github.com/repos/{repo}/releases/tags/{tag}"))?;
    let assets = assets_of(&rel);
    let ksud_asset = pick_ksud(&assets).ok_or_else(|| "Bu sürümde bu bilgisayar için ksud bulunamadı.".to_string())?;
    let dir = cache_dir(repo, tag)?;
    let ksud = ensure_asset(app, repo, &dir, ksud_asset)?;
    make_executable(&ksud)?;
    let ko_name = format!("{kmi}_kernelsu.ko");
    let ko = match assets.iter().find(|a| a.name == ko_name) {
        Some(a) => Some(ensure_asset(app, repo, &dir, a)?),
        None => { emit_log(app, &format!("{ko_name} sürümde yok; ksud içindeki gömülü modül denenecek."), "info"); None }
    };
    emit_step(app, "download", "done", &ksud_asset.name);

    // 2) work out which flags this ksud understands (they drift between forks/versions)
    emit_step(app, "patch", "running", &format!("{partition}.img · {kmi}"));
    let help = match Command::new(&ksud).args(["boot-patch", "--help"]).stdin(Stdio::null()).output() {
        Ok(o) => format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)),
        Err(e) => return Err(format!("ksud çalıştırılamadı: {e}")),
    };
    let has = |f: &str| help.contains(f);
    if !has("boot-patch") && !has("--boot") { return Err("Bu ksud sürümü 'boot-patch' komutunu desteklemiyor.".into()); }
    let outdir = PathBuf::from(folder).join("ksu-patched");
    fs::create_dir_all(&outdir).map_err(|e| format!("Çıktı klasörü oluşturulamadı: {e}"))?;
    let started = SystemTime::now();

    let mut magiskboot: Option<PathBuf> = None;
    let (mut success, mut last_tail) = (false, Vec::new());
    for attempt in 0..2 {
        let mut cmd = Command::new(&ksud);
        cmd.current_dir(&outdir).arg("boot-patch").arg(if has("--boot") { "--boot" } else { "-b" }).arg(&img);
        if has("--kmi") { cmd.arg("--kmi").arg(kmi); }
        if let Some(k) = &ko { if has("--lkm") { cmd.arg("--lkm").arg(k); } else if has("--module") { cmd.arg("--module").arg(k); } }
        if let Some(m) = &magiskboot { cmd.arg("--magiskboot").arg(m); }
        emit_log(app, &format!("ksud boot-patch {partition}.img (KMI {kmi})"), "info");
        let (ok, tail) = run_logged(app, cmd)?;
        if ok { success = true; break; }
        let text = tail.join("\n").to_lowercase();
        last_tail = tail;
        // older ksud builds shell out to magiskboot: fetch the one published next to it, once
        if attempt == 0 && text.contains("magiskboot") {
            if let Some(a) = pick_magiskboot(&assets) { let p = ensure_asset(app, repo, &dir, a)?; make_executable(&p)?; magiskboot = Some(p); continue; }
            if let Some(p) = find_tool("magiskboot") { magiskboot = Some(p); continue; }
        }
        break;
    }
    if !success {
        emit_step(app, "patch", "failed", "");
        return Err(format!("ksud boot-patch başarısız:\n{}", last_tail.iter().rev().take(6).rev().cloned().collect::<Vec<_>>().join("\n")));
    }

    // 3) pick up the file ksud wrote and give it a descriptive name
    let produced = fs::read_dir(&outdir).map_err(|e| e.to_string())?.flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("img") && e.metadata().and_then(|m| m.modified()).map(|t| t >= started).unwrap_or(false))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok()).map(|e| e.path())
        .ok_or_else(|| "ksud çıktı dosyası bulunamadı.".to_string())?;
    let safe_repo = repo.replace('/', "_");
    let final_path = outdir.join(format!("{partition}_{safe_repo}_{tag}_{kmi}.img"));
    fs::rename(&produced, &final_path).or_else(|_| fs::copy(&produced, &final_path).map(|_| ())).map_err(|e| format!("Çıktı adlandırılamadı: {e}"))?;
    let size = fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);
    emit_step(app, "patch", "done", &final_path.to_string_lossy());
    emit_log(app, &format!("Yamalı imaj hazır: {}", final_path.display()), "success");
    Ok(json!({"path": final_path.to_string_lossy(), "partition": partition, "kmi": kmi, "tag": tag, "repo": repo, "size": size}))
}

// ---------------------------------------------------------------- flashing
#[tauri::command]
pub async fn ksu_flash(app: AppHandle, serial: String, partition: String, file: String, vbmeta: Option<String>, reboot: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || ksu_flash_blocking(&app, &serial, &partition, &file, vbmeta.as_deref(), reboot)).await.map_err(|e| e.to_string())?
}

fn ksu_flash_blocking(app: &AppHandle, serial: &str, partition: &str, file: &str, vbmeta: Option<&str>, reboot: bool) -> Result<String, String> {
    if !ok_serial(serial) { return Err("Geçersiz seri numarası.".into()); }
    if partition != "init_boot" && partition != "boot" { return Err("Bölüm init_boot veya boot olmalı.".into()); }
    let path = PathBuf::from(file);
    if !path.is_file() || path.extension().and_then(|x| x.to_str()) != Some("img") { return Err("Yamalı imaj dosyası bulunamadı.".into()); }
    let dev = list_devices_blocking()?.into_iter().find(|d| d.serial == serial).ok_or_else(|| "Hedef cihaz bağlı değil.".to_string())?;
    if dev.mode != "Bootloader Fastboot" && dev.mode != "Fastbootd" {
        return Err(format!("Cihaz Fastboot modunda değil (algılanan: {}).", dev.mode));
    }
    let file_s = path.to_string_lossy().into_owned();
    emit_step(app, "flash", "running", partition);
    if let Some(vb) = vbmeta {
        let vp = PathBuf::from(vb);
        if !vp.is_file() { return Err("vbmeta.img bulunamadı.".into()); }
        let vs = vp.to_string_lossy().into_owned();
        emit_log(app, "vbmeta doğrulaması kapatılıyor (verity + verification)…", "info");
        let o = run_output("fastboot", &["-s", serial, "--disable-verity", "--disable-verification", "flash", "vbmeta", vs.as_str()])?;
        if !o.status.success() { emit_step(app, "flash", "failed", ""); return Err(format!("vbmeta flash başarısız: {}", command_text(&o))); }
    }
    let o = run_output("fastboot", &["-s", serial, "flash", partition, file_s.as_str()])?;
    let text = command_text(&o);
    if !o.status.success() { emit_step(app, "flash", "failed", ""); return Err(format!("{partition} flash başarısız: {text}")); }
    emit_log(app, &format!("{partition} flashlandı."), "success");
    emit_step(app, "flash", "done", partition);
    if reboot {
        emit_step(app, "reboot", "running", "");
        let r = run_output("fastboot", &["-s", serial, "reboot"])?;
        if !r.status.success() { emit_step(app, "reboot", "failed", ""); return Err(format!("Yeniden başlatma başarısız: {}", command_text(&r))); }
        emit_step(app, "reboot", "done", "");
    }
    Ok(format!("{partition} yamalı imajla flashlandı."))
}

// ---------------------------------------------------------------- manager app (root app)
fn wait_for_android(app: &AppHandle, serial: &str, secs: u64) -> Result<(), String> {
    emit_log(app, "Cihazın açılması bekleniyor…", "info");
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let up = list_devices_blocking().map(|d| d.iter().any(|x| x.serial == serial && x.mode.starts_with("ADB"))).unwrap_or(false);
        if up {
            if let Ok(o) = run_output("adb", &["-s", serial, "shell", "getprop", "sys.boot_completed"]) {
                if command_text(&o).trim() == "1" { return Ok(()); }
            }
        }
        if Instant::now() >= deadline { return Err("Cihaz zamanında açılmadı veya ADB bağlantısı yok. USB hata ayıklamanın açık olduğundan emin ol.".into()); }
        thread::sleep(Duration::from_secs(3));
    }
}

fn third_party_packages(serial: &str) -> Vec<String> {
    run_output("adb", &["-s", serial, "shell", "pm", "list", "packages", "-3"]).map(|o| {
        command_text(&o).lines().filter_map(|l| l.trim().strip_prefix("package:").map(|p| p.trim().to_string())).collect()
    }).unwrap_or_default()
}

/// Downloads the release's manager APK and installs it over ADB (optionally waiting for the phone to boot).
#[tauri::command]
pub async fn ksu_install_manager(app: AppHandle, serial: String, repo: String, tag: String, spoofed: bool, wait_boot: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || ksu_install_manager_blocking(&app, &serial, &repo, &tag, spoofed, wait_boot)).await.map_err(|e| e.to_string())?
}

fn ksu_install_manager_blocking(app: &AppHandle, serial: &str, repo: &str, tag: &str, spoofed: bool, wait_boot: bool) -> Result<String, String> {
    if !ok_serial(serial) { return Err("Geçersiz seri numarası.".into()); }
    if !ok_repo(repo) || !ok_tag(tag) { return Err("Geçersiz depo veya sürüm.".into()); }
    emit_step(app, "manager", "running", "");
    let result = (|| -> Result<String, String> {
        let rel = github_json(&format!("https://api.github.com/repos/{repo}/releases/tags/{tag}"))?;
        let assets = assets_of(&rel);
        let apk_asset = pick_manager(&assets, spoofed).ok_or_else(|| "Bu sürümde yönetici APK dosyası bulunamadı.".to_string())?;
        let dir = cache_dir(repo, tag)?;
        let apk = ensure_asset(app, repo, &dir, apk_asset)?;
        if wait_boot { wait_for_android(app, serial, 300)?; }
        else {
            let dev = list_devices_blocking()?.into_iter().find(|d| d.serial == serial).ok_or_else(|| "Hedef cihaz bağlı değil.".to_string())?;
            if !dev.mode.starts_with("ADB") { return Err(format!("Cihaz Android (ADB) modunda değil (algılanan: {}).", dev.mode)); }
        }
        let before = third_party_packages(serial);
        emit_log(app, &format!("Yönetici uygulaması kuruluyor: {}", apk_asset.name), "info");
        let apk_s = apk.to_string_lossy().into_owned();
        let out = run_output("adb", &["-s", serial, "install", "-r", apk_s.as_str()])?;
        let text = command_text(&out);
        if !text.contains("Success") { return Err(format!("APK kurulamadı: {}", text.lines().last().unwrap_or("").trim())); }
        // open it once so the root manager registers itself
        let new_pkg = third_party_packages(serial).into_iter().find(|p| !before.contains(p));
        let pkg = new_pkg.or_else(|| ["com.rifsxd.ksunext", "me.weishu.kernelsu"].iter().map(|s| s.to_string()).find(|p| third_party_packages(serial).contains(p)));
        if let Some(p) = pkg.filter(|p| p.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')) {
            let _ = run_output("adb", &["-s", serial, "shell", "monkey", "-p", p.as_str(), "-c", "android.intent.category.LAUNCHER", "1"]);
            return Ok(format!("Yönetici uygulaması kuruldu ({p}) ve açıldı."));
        }
        Ok("Yönetici uygulaması kuruldu.".to_string())
    })();
    match &result { Ok(_) => emit_step(app, "manager", "done", ""), Err(_) => emit_step(app, "manager", "failed", "") }
    result
}
