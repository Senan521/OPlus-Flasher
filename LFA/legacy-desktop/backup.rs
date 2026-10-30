//! App backup / restore over ADB.
//!
//! Approach (modelled on Android-DataBackup / speed-backup, driven from the PC instead of an on-device app):
//!  * root is obtained through the root manager's `su` reachable from `adb shell` (Magisk, KernelSU, APatch)
//!    or because adbd itself runs as root (`adb root`);
//!  * per app we archive: APK(s), /data/user/N/<pkg>, /data/user_de/N/<pkg>, and the external
//!    Android/{data,obb,media}/<pkg> directories (cache dirs are excluded);
//!  * archives are streamed through `adb exec-out su -c "tar -c ..."` straight into zstd/gzip on the PC;
//!  * restore installs the APK(s), force-stops the app, extracts the data, fixes ownership (uid:gid),
//!    runs `restorecon -RF` for SELinux labels and re-grants runtime permissions.
use super::{find_tool, home_dir, list_devices_blocking};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

static BUSY: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

struct BusyGuard;
impl BusyGuard {
    fn acquire() -> Result<Self, String> {
        if BUSY.swap(true, Ordering::SeqCst) { return Err("Başka bir yedekleme/geri yükleme işlemi sürüyor.".into()); }
        CANCEL.store(false, Ordering::SeqCst);
        Ok(BusyGuard)
    }
}
impl Drop for BusyGuard { fn drop(&mut self) { BUSY.store(false, Ordering::SeqCst); } }

fn ok_serial(s: &str) -> bool { !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-')) }
fn ok_pkg(s: &str) -> bool { !s.is_empty() && s.len() <= 255 && !s.starts_with('.') && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') }
fn ok_remote_path(s: &str) -> bool { !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '~' | '=' | '+' | '-' | '@')) }
fn ok_file_name(s: &str) -> bool { !s.is_empty() && !s.contains('/') && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) }
fn adb() -> Result<PathBuf, String> { find_tool("adb").ok_or_else(|| "adb bulunamadı. Ayarlar → Gerekli araçlar bölümünden kurabilirsin.".to_string()) }

fn emit_log(app: &AppHandle, text: &str, kind: &str) { let _ = app.emit("backup-log", json!({"text": text, "kind": kind})); }
fn emit_progress(app: &AppHandle, mode: &str, pkg: &str, step: &str, index: usize, total: usize, percent: u32, state: &str) {
    let _ = app.emit("backup-progress", json!({"mode": mode, "pkg": pkg, "step": step, "index": index, "total": total, "percent": percent, "state": state}));
}

/// Runs `adb -s <serial> shell <cmd>` with a timeout; returns (success, combined output).
fn adb_shell(serial: &str, cmd: &str, secs: u64) -> Result<(bool, String), String> {
    let mut child = Command::new(adb()?).args(["-s", serial, "shell", cmd])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|e| format!("adb başlatılamadı: {e}"))?;
    let read = |p: Option<Box<dyn Read + Send>>| thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(mut r) = p { let _ = r.read_to_end(&mut b); }
        String::from_utf8_lossy(&b).into_owned()
    });
    let h_out = read(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let h_err = read(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let deadline = Instant::now() + Duration::from_secs(secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if Instant::now() >= deadline { let _ = child.kill(); let _ = child.wait(); break None; }
                thread::sleep(Duration::from_millis(80));
            }
            Err(e) => return Err(format!("Süreç beklenemedi: {e}")),
        }
    };
    let (o, e) = (h_out.join().unwrap_or_default(), h_err.join().unwrap_or_default());
    match status {
        Some(s) => Ok((s.success(), format!("{o}{e}").trim().to_string())),
        None => Err(format!("Zaman aşımı ({secs} sn): {cmd}")),
    }
}

/// Wraps a command so it runs as root. `cmd` must not contain single quotes.
fn rooted(method: &str, cmd: &str) -> Result<String, String> {
    if cmd.contains('\'') { return Err("Geçersiz komut.".into()); }
    Ok(if method == "adbroot" { cmd.to_string() } else { format!("su -c '{cmd}'") })
}

// ---------------------------------------------------------------- root check
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootStatus { ok: bool, method: String, detail: String, sdk: u32, android: String, users: Vec<String>, has_tar: bool }

#[tauri::command]
pub async fn root_check(serial: String) -> Result<RootStatus, String> {
    tauri::async_runtime::spawn_blocking(move || root_check_blocking(&serial)).await.map_err(|e| e.to_string())?
}

pub fn root_check_blocking(serial: &str) -> Result<RootStatus, String> {
    if !ok_serial(serial) { return Err("Geçersiz seri numarası.".into()); }
    let dev = list_devices_blocking()?.into_iter().find(|d| d.serial == serial).ok_or_else(|| "Hedef cihaz bağlı değil.".to_string())?;
    let mut st = RootStatus { ok: false, method: "none".into(), detail: String::new(), sdk: 0, android: dev.android.clone(), users: vec![], has_tar: false };
    if !dev.mode.starts_with("ADB") {
        st.detail = format!("Cihaz ADB modunda değil ({}). Android açık ve USB hata ayıklama etkin olmalı.", dev.mode);
        return Ok(st);
    }
    st.sdk = adb_shell(serial, "getprop ro.build.version.sdk", 8).ok().and_then(|(_, o)| o.trim().parse().ok()).unwrap_or(0);
    if let Ok((_, o)) = adb_shell(serial, "id", 8) {
        if o.contains("uid=0(") { st.ok = true; st.method = "adbroot".into(); st.detail = "adbd root olarak çalışıyor (adb root).".into(); }
    }
    if !st.ok {
        match adb_shell(serial, "su -c id", 25) {
            Ok((_, o)) if o.contains("uid=0(") => { st.ok = true; st.method = "su".into(); st.detail = "ADB shell root erişimi onaylandı (su).".into(); }
            Ok((_, o)) => {
                let line = o.lines().next().unwrap_or("").trim();
                st.detail = if line.is_empty() { "su yanıt vermedi veya root yok.".into() } else { format!("Root reddedildi: {line}") };
            }
            Err(e) => st.detail = format!("{e} — cihazdaki root uygulamasında (Magisk/KernelSU/APatch) “Shell” için izin isteği çıkmış olabilir."),
        }
    }
    if st.ok {
        let probe = rooted(&st.method, "command -v tar")?;
        st.has_tar = adb_shell(serial, &probe, 10).map(|(ok, o)| ok && o.contains("tar")).unwrap_or(false);
        if !st.has_tar { st.ok = false; st.detail = "Cihazda tar komutu bulunamadı; yedekleme yapılamaz.".into(); }
    }
    if let Ok((_, o)) = adb_shell(serial, "pm list users", 10) {
        st.users = o.lines().filter_map(|l| l.split("UserInfo{").nth(1).and_then(|r| r.split(':').next()).map(|s| s.to_string())).collect();
    }
    Ok(st)
}

// ---------------------------------------------------------------- package list
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo { package: String, version_code: String, uid: String, apk_path: String, is_system: bool }

#[tauri::command]
pub async fn list_packages(serial: String, user_id: u32, include_system: bool) -> Result<Vec<AppInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || list_packages_blocking(&serial, user_id, include_system)).await.map_err(|e| e.to_string())?
}

fn list_packages_blocking(serial: &str, user: u32, include_system: bool) -> Result<Vec<AppInfo>, String> {
    if !ok_serial(serial) { return Err("Geçersiz seri numarası.".into()); }
    let filter = if include_system { "" } else { "-3" };
    let (mut ok, mut out) = adb_shell(serial, &format!("pm list packages -f -U --show-versioncode --user {user} {filter}"), 40)?;
    if !ok || out.contains("Unknown option") || out.contains("Error") {
        let r = adb_shell(serial, &format!("pm list packages -f --user {user} {filter}"), 40)?;
        ok = r.0; out = r.1;
    }
    if !ok && out.is_empty() { return Err("Paket listesi alınamadı.".into()); }
    let mut apps: Vec<AppInfo> = out.lines().filter_map(|l| {
        let rest = l.trim().strip_prefix("package:")?;
        let mut tok = rest.split_whitespace();
        let (path, pkg) = tok.next()?.rsplit_once('=')?;
        if !ok_pkg(pkg) { return None; }
        let (mut vc, mut uid) = (String::new(), String::new());
        for t in tok { if let Some(v) = t.strip_prefix("versionCode:") { vc = v.into(); } else if let Some(u) = t.strip_prefix("uid:") { uid = u.into(); } }
        let sys = ["/system", "/product", "/vendor", "/apex", "/system_ext", "/odm"].iter().any(|p| path.starts_with(p));
        Some(AppInfo { package: pkg.into(), version_code: vc, uid, apk_path: path.into(), is_system: sys })
    }).collect();
    apps.sort_by(|a, b| a.package.cmp(&b.package));
    Ok(apps)
}


// ---------------------------------------------------------------- app icons
// Launcher icons are read straight out of the APK on the device without pulling the whole file:
// the zip central directory is read with `tail -c | head -c`, then either a well-named ic_launcher
// image is picked, or AndroidManifest.xml + resources.arsc are parsed to resolve the real icon
// resource (incl. adaptive icons: PNG/WebP foreground + background colour). As a last resort the
// icon can be fetched from the public Google Play page (opt-in, setting "online icons").
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64(data: &[u8]) -> String {
    let mut o = String::with_capacity(data.len() * 4 / 3 + 4);
    for ch in data.chunks(3) {
        let n = ((ch[0] as u32) << 16) | ((*ch.get(1).unwrap_or(&0) as u32) << 8) | (*ch.get(2).unwrap_or(&0) as u32);
        o.push(B64[((n >> 18) & 63) as usize] as char);
        o.push(B64[((n >> 12) & 63) as usize] as char);
        o.push(if ch.len() > 1 { B64[((n >> 6) & 63) as usize] as char } else { '=' });
        o.push(if ch.len() > 2 { B64[(n & 63) as usize] as char } else { '=' });
    }
    o
}

fn img_kind(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { Some(("image/png", "png")) }
    else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" { Some(("image/webp", "webp")) }
    else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) { Some(("image/jpeg", "jpg")) }
    else { None }
}

fn data_uri(bytes: &[u8]) -> Option<String> {
    let (mime, _) = img_kind(bytes)?;
    Some(format!("data:{mime};base64,{}", b64(bytes)))
}

fn le16(b: &[u8], o: usize) -> usize { (b[o] as usize) | ((b[o + 1] as usize) << 8) }
fn le32(b: &[u8], o: usize) -> u64 { (b[o] as u64) | ((b[o + 1] as u64) << 8) | ((b[o + 2] as u64) << 16) | ((b[o + 3] as u64) << 24) }
fn r16(b: &[u8], o: usize) -> Option<usize> { Some((*b.get(o)? as usize) | ((*b.get(o + 1)? as usize) << 8)) }
fn r32(b: &[u8], o: usize) -> Option<u32> { Some((*b.get(o)? as u32) | ((*b.get(o + 1)? as u32) << 8) | ((*b.get(o + 2)? as u32) << 16) | ((*b.get(o + 3)? as u32) << 24)) }

fn remote_bytes(adbp: &Path, serial: &str, cmd: &str) -> Result<Vec<u8>, String> {
    let out = Command::new(adbp).args(["-s", serial, "exec-out", cmd]).stdin(Stdio::null()).stderr(Stdio::null()).output()
        .map_err(|e| format!("adb başlatılamadı: {e}"))?;
    Ok(out.stdout)
}
fn remote_range(adbp: &Path, serial: &str, path: &str, start: u64, len: u64) -> Result<Vec<u8>, String> {
    remote_bytes(adbp, serial, &format!("tail -c +{} {path} | head -c {len}", start + 1))
}

struct ZipEnt { name: String, method: usize, crc: u64, csize: u64, usize_: u64, off: u64 }

fn read_central(adbp: &Path, serial: &str, path: &str) -> Result<Vec<ZipEnt>, String> {
    let (_, sz) = adb_shell(serial, &format!("stat -c %s {path}"), 10)?;
    let size: u64 = sz.trim().parse().map_err(|_| "APK boyutu okunamadı.".to_string())?;
    let tail = remote_bytes(adbp, serial, &format!("tail -c {} {path}", size.min(65557)))?;
    if tail.len() < 22 { return Err("APK sonu okunamadı.".into()); }
    let pos = (0..=tail.len() - 22).rev().find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06]).ok_or_else(|| "Zip bitiş kaydı bulunamadı.".to_string())?;
    let (cd_size, cd_off) = (le32(&tail, pos + 12), le32(&tail, pos + 16));
    if cd_off == 0xFFFF_FFFF || cd_size == 0 || cd_size > 16 * 1024 * 1024 || cd_off + cd_size > size { return Err("Desteklenmeyen zip yapısı.".into()); }
    let cd = remote_range(adbp, serial, path, cd_off, cd_size)?;
    let (mut ents, mut p) = (Vec::new(), 0usize);
    while p + 46 <= cd.len() && le32(&cd, p) == 0x0201_4b50 {
        let (nl, el, cl) = (le16(&cd, p + 28), le16(&cd, p + 30), le16(&cd, p + 32));
        if p + 46 + nl > cd.len() { break; }
        ents.push(ZipEnt {
            name: String::from_utf8_lossy(&cd[p + 46..p + 46 + nl]).into_owned(), method: le16(&cd, p + 10), crc: le32(&cd, p + 16),
            csize: le32(&cd, p + 20), usize_: le32(&cd, p + 24), off: le32(&cd, p + 42),
        });
        p += 46 + nl + el + cl;
    }
    Ok(ents)
}

fn inflate_raw(raw: &[u8], crc: u64, size: u64) -> Result<Vec<u8>, String> {
    let gz = find_tool("gzip").ok_or_else(|| "gzip bulunamadı.".to_string())?;
    let mut data = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    data.extend_from_slice(raw);
    data.extend_from_slice(&(crc as u32).to_le_bytes());
    data.extend_from_slice(&(size as u32).to_le_bytes());
    let mut c = Command::new(gz).arg("-dc").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    let mut si = c.stdin.take().ok_or_else(|| "gzip stdin açılamadı.".to_string())?;
    let w = thread::spawn(move || { let _ = si.write_all(&data); });
    let mut out = Vec::new();
    if let Some(mut so) = c.stdout.take() { let _ = so.read_to_end(&mut out); }
    let _ = w.join();
    let st = c.wait().map_err(|e| e.to_string())?;
    if !st.success() || out.len() as u64 != size { return Err("Simge açılamadı.".into()); }
    Ok(out)
}

fn read_zip_entry(adbp: &Path, serial: &str, apk: &str, e: &ZipEnt) -> Result<Vec<u8>, String> {
    let hdr = remote_range(adbp, serial, apk, e.off, 30)?;
    if hdr.len() < 30 || le32(&hdr, 0) != 0x0403_4b50 { return Err("Zip girdisi okunamadı.".into()); }
    let data_off = e.off + 30 + le16(&hdr, 26) as u64 + le16(&hdr, 28) as u64;
    let raw = remote_range(adbp, serial, apk, data_off, e.csize)?;
    if raw.len() as u64 != e.csize { return Err("Simge verisi eksik.".into()); }
    match e.method {
        0 => Ok(raw),
        8 => inflate_raw(&raw, e.crc, e.usize_),
        _ => Err("Desteklenmeyen sıkıştırma.".into()),
    }
}

/// Best conventionally-named launcher image inside the APK, with a quality score.
fn pick_icon(ents: &[ZipEnt]) -> Option<(i32, &ZipEnt)> {
    ents.iter().filter(|e| e.usize_ > 0 && e.usize_ < 600_000 && e.name.starts_with("res/") && (e.name.ends_with(".png") || e.name.ends_with(".webp")))
        .filter_map(|e| {
            let base = e.name.rsplit('/').next().unwrap_or("").to_lowercase();
            let stem = base.rsplit_once('.')?.0.to_string();
            if ["background", "monochrome", "notification", "badge", "splash", "banner"].iter().any(|w| stem.contains(w)) { return None; }
            let mut s: i32 = if stem == "ic_launcher" { 100 } else if stem == "ic_launcher_round" { 80 } else if stem.contains("launcher") && !stem.contains("foreground") { 60 }
                else if stem == "icon" || stem == "app_icon" || stem == "ic_app" { 55 }
                else if stem.contains("launcher") && stem.contains("foreground") { 45 } else { return None };
            let n = e.name.as_str();
            s += if n.contains("xxhdpi") { 6 } else if n.contains("xxxhdpi") { 5 } else if n.contains("xhdpi") { 4 } else if n.contains("hdpi") { 3 } else if n.contains("mdpi") { 2 } else { 1 };
            Some((s, e))
        }).max_by_key(|(s, _)| *s)
}

// ---- minimal binary-XML / resources.arsc readers (just enough to resolve a launcher icon) ----
fn pool_string(b: &[u8], at: usize, utf8: bool) -> Option<String> {
    if utf8 {
        let mut p = at;
        let l1 = *b.get(p)? as usize;
        p += if l1 & 0x80 != 0 { 2 } else { 1 };
        let l2b = *b.get(p)? as usize;
        let (l2, adv) = if l2b & 0x80 != 0 { ((((l2b & 0x7f) << 8) | (*b.get(p + 1)? as usize)), 2) } else { (l2b, 1) };
        p += adv;
        Some(String::from_utf8_lossy(b.get(p..p + l2)?).into_owned())
    } else {
        let l0 = r16(b, at)?;
        let (l, off) = if l0 & 0x8000 != 0 { (((l0 & 0x7fff) << 16) | r16(b, at + 2)?, 4) } else { (l0, 2) };
        if l > 4096 { return None; }
        let mut v: Vec<u16> = Vec::with_capacity(l);
        for i in 0..l { v.push(r16(b, at + off + i * 2)? as u16); }
        Some(String::from_utf16_lossy(&v))
    }
}

fn parse_pool(b: &[u8], start: usize) -> Option<Vec<String>> {
    if r16(b, start)? != 1 { return None; }
    let hs = r16(b, start + 2)?;
    let n = r32(b, start + 8)? as usize;
    let flags = r32(b, start + 16)?;
    let ss = r32(b, start + 20)? as usize;
    if n > 300_000 { return None; }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let off = r32(b, start + hs + i * 4)? as usize;
        out.push(pool_string(b, start + ss + off, flags & 0x100 != 0).unwrap_or_default());
    }
    Some(out)
}

struct XElem { name: String, attrs: Vec<(String, u8, u32)> }

fn parse_axml(b: &[u8]) -> Option<Vec<XElem>> {
    if r16(b, 0)? != 3 { return None; }
    let mut p = r16(b, 2)?;
    let (mut pool, mut out) = (Vec::<String>::new(), Vec::new());
    while p + 8 <= b.len() {
        let ty = r16(b, p)?;
        let sz = r32(b, p + 4)? as usize;
        if sz < 8 { break; }
        if ty == 1 { pool = parse_pool(b, p)?; }
        else if ty == 0x0102 {
            let name_i = r32(b, p + 20)? as usize;
            let (astart, asize, acount) = (r16(b, p + 24)?, r16(b, p + 26)?, r16(b, p + 28)?);
            let mut attrs = Vec::new();
            for i in 0..acount.min(64) {
                let a = p + 16 + astart + i * asize;
                let nm = r32(b, a + 4)? as usize;
                attrs.push((pool.get(nm).cloned().unwrap_or_default(), *b.get(a + 15)?, r32(b, a + 16)?));
            }
            out.push(XElem { name: pool.get(name_i).cloned().unwrap_or_default(), attrs });
        }
        p += sz;
    }
    Some(out)
}

struct Val { density: usize, sdk: usize, dtype: u8, data: u32 }
enum Res { Path(String), Color(u32) }

struct Arsc<'a> { b: &'a [u8], pool: Vec<String>, types: Vec<(u32, u8, usize)> }

fn parse_arsc(b: &[u8]) -> Option<Arsc<'_>> {
    if r16(b, 0)? != 2 { return None; }
    let hs = r16(b, 2)?;
    let pool = parse_pool(b, hs)?;
    let mut p = hs + r32(b, hs + 4)? as usize;
    let mut types = Vec::new();
    while p + 8 <= b.len() {
        let ty = r16(b, p)?;
        let sz = r32(b, p + 4)? as usize;
        if sz < 8 { break; }
        if ty == 0x0200 {
            let (pid, phs) = (r32(b, p + 8)?, r16(b, p + 2)?);
            let end = (p + sz).min(b.len());
            let mut q = p + phs;
            while q + 8 <= end {
                let qt = r16(b, q)?;
                let qs = r32(b, q + 4)? as usize;
                if qs < 8 { break; }
                if qt == 0x0201 { types.push((pid, *b.get(q + 8)?, q)); }
                q += qs;
            }
        }
        p += sz;
    }
    Some(Arsc { b, pool, types })
}

impl Arsc<'_> {
    fn lookup(&self, id: u32) -> Vec<Val> {
        let (pkg, ty, ent) = (id >> 24, ((id >> 16) & 0xff) as u8, (id & 0xffff) as usize);
        let b = self.b;
        let mut out = Vec::new();
        for &(p, t, q) in &self.types {
            if p != pkg || t != ty { continue; }
            let (Some(hs), Some(cnt), Some(es)) = (r16(b, q + 2), r32(b, q + 12), r32(b, q + 16)) else { continue };
            let flags = b.get(q + 9).copied().unwrap_or(0);
            let off: Option<usize> = if flags & 1 != 0 {
                (0..cnt as usize).find_map(|i| r32(b, q + hs + i * 4).filter(|v| (*v & 0xffff) as usize == ent).map(|v| ((v >> 16) as usize) * 4))
            } else if ent < cnt as usize {
                r32(b, q + hs + ent * 4).filter(|v| *v != 0xFFFF_FFFF).map(|v| v as usize)
            } else { None };
            let Some(off) = off else { continue };
            let e = q + es as usize + off;
            let Some(eflags) = r16(b, e + 2) else { continue };
            if eflags & 1 != 0 { continue; }
            let (Some(dtype), Some(data)) = (b.get(e + 11).copied(), r32(b, e + 12)) else { continue };
            out.push(Val { density: r16(b, q + 20 + 14).unwrap_or(0), sdk: r16(b, q + 20 + 24).unwrap_or(0), dtype, data });
        }
        out
    }

    fn resolve(&self, id: u32, depth: u8) -> Option<Res> {
        if depth > 4 { return None; }
        let (mut best, mut xml): (Option<(i32, String)>, Option<(usize, String)>) = (None, None);
        for v in self.lookup(id) {
            match v.dtype {
                3 => {
                    if let Some(s) = self.pool.get(v.data as usize) {
                        let l = s.to_lowercase();
                        if l.ends_with(".png") || l.ends_with(".webp") {
                            let sc = match v.density { 480 => 6, 640 => 5, 320 => 4, 240 => 3, 160 | 0xFFFF => 2, 120 => 1, _ => 0 };
                            if best.as_ref().map(|x| sc > x.0).unwrap_or(true) { best = Some((sc, s.clone())); }
                        } else if l.ends_with(".xml") && xml.as_ref().map(|x| v.sdk >= x.0).unwrap_or(true) { xml = Some((v.sdk, s.clone())); }
                    }
                }
                0x1c | 0x1d => return Some(Res::Color(v.data)),
                1 => { if best.is_none() && xml.is_none() { if let Some(r) = self.resolve(v.data, depth + 1) { return Some(r); } } }
                _ => {}
            }
        }
        if let Some((_, p)) = best { return Some(Res::Path(p)); }
        xml.map(|(_, p)| Res::Path(p))
    }
}

pub struct IconOut { bytes: Vec<u8>, bg: Option<String>, adaptive: bool }

fn is_raster(p: &str) -> bool { let l = p.to_lowercase(); l.ends_with(".png") || l.ends_with(".webp") }

fn drawable_id(elems: &[XElem], tag: &str) -> Option<u32> {
    let i = elems.iter().position(|e| e.name == tag)?;
    for e in elems.iter().skip(i).take(2) {
        if let Some(a) = e.attrs.iter().find(|a| (a.0 == "drawable" || a.0 == "src") && a.1 == 1) { return Some(a.2); }
    }
    None
}

/// Resolve AndroidManifest <application android:icon> through resources.arsc.
fn icon_via_resources(adbp: &Path, serial: &str, apk: &str, ents: &[ZipEnt]) -> Result<IconOut, String> {
    let find = |n: &str| ents.iter().find(|e| e.name == n);
    let (man, arsc_e) = (find("AndroidManifest.xml"), find("resources.arsc"));
    let (Some(man), Some(arsc_e)) = (man, arsc_e) else { return Err("Manifest veya kaynak tablosu yok.".into()) };
    if arsc_e.usize_ > 48 * 1024 * 1024 { return Err("Kaynak tablosu çok büyük.".into()); }
    let manifest = parse_axml(&read_zip_entry(adbp, serial, apk, man)?).ok_or_else(|| "Manifest okunamadı.".to_string())?;
    let app = manifest.iter().find(|e| e.name == "application").ok_or_else(|| "application öğesi yok.".to_string())?;
    let arsc_bytes = read_zip_entry(adbp, serial, apk, arsc_e)?;
    let arsc = parse_arsc(&arsc_bytes).ok_or_else(|| "Kaynak tablosu okunamadı.".to_string())?;
    let fetch = |path: &str| -> Result<Vec<u8>, String> {
        let e = find(path).ok_or_else(|| "Simge dosyası zipte yok.".to_string())?;
        read_zip_entry(adbp, serial, apk, e)
    };
    for attr in ["icon", "roundIcon"] {
        let Some(a) = app.attrs.iter().find(|a| a.0 == attr && a.1 == 1) else { continue };
        match arsc.resolve(a.2, 0) {
            Some(Res::Path(p)) if is_raster(&p) => return Ok(IconOut { bytes: fetch(&p)?, bg: None, adaptive: false }),
            Some(Res::Path(p)) => {
                // adaptive-icon XML: take the raster foreground and (if any) a solid background colour
                let Ok(xml) = fetch(&p) else { continue };
                let Some(elems) = parse_axml(&xml) else { continue };
                let Some(fg_id) = drawable_id(&elems, "foreground") else { continue };
                let Some(Res::Path(fp)) = arsc.resolve(fg_id, 0) else { continue };
                if !is_raster(&fp) { continue; }
                let bg = drawable_id(&elems, "background").and_then(|id| arsc.resolve(id, 0)).and_then(|r| match r { Res::Color(c) => Some(format!("#{:06x}", c & 0xFFFFFF)), _ => None });
                return Ok(IconOut { bytes: fetch(&fp)?, bg, adaptive: true });
            }
            _ => {}
        }
    }
    Err("Simge çözülemedi.".into())
}

fn fetch_icon(adbp: &Path, serial: &str, apk_path: &str) -> Result<IconOut, String> {
    if !ok_remote_path(apk_path) { return Err("Geçersiz APK yolu.".into()); }
    let ents = read_central(adbp, serial, apk_path)?;
    let by_name = pick_icon(&ents);
    // 1) a well-named launcher image is the cheapest and usually correct
    if let Some((score, e)) = by_name {
        if score >= 60 {
            if let Ok(bytes) = read_zip_entry(adbp, serial, apk_path, e) { if img_kind(&bytes).is_some() { return Ok(IconOut { bytes, bg: None, adaptive: false }); } }
        }
    }
    // 2) resolve the real icon resource from the manifest / resource table
    match icon_via_resources(adbp, serial, apk_path, &ents) {
        Ok(o) if img_kind(&o.bytes).is_some() => return Ok(o),
        _ => {}
    }
    // 3) weaker name matches (e.g. only a foreground layer)
    if let Some((_, e)) = by_name {
        if let Ok(bytes) = read_zip_entry(adbp, serial, apk_path, e) { if img_kind(&bytes).is_some() { return Ok(IconOut { bytes, bg: None, adaptive: false }); } }
    }
    Err("Simge bulunamadı.".into())
}

fn fetch_url(url: &str) -> Result<Vec<u8>, String> {
    let ua = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";
    let run = if let Some(curl) = find_tool("curl") {
        Command::new(curl).args(["-fsSL", "--max-time", "15", "-A", ua, "-H", "Cookie: CONSENT=YES+cb", "-H", "Accept-Language: en-US,en;q=0.9", url]).stdin(Stdio::null()).output()
    } else if let Some(wget) = find_tool("wget") {
        Command::new(wget).args(["-qO-", "-T", "15", "-U", ua, "--header=Cookie: CONSENT=YES+cb", url]).stdin(Stdio::null()).output()
    } else {
        return Err("İndirme için curl veya wget gerekli.".into());
    };
    let out = run.map_err(|e| format!("İndirme başlatılamadı: {e}"))?;
    if !out.status.success() { return Err(format!("İndirme başarısız: {url}")); }
    Ok(out.stdout)
}

/// Opt-in fallback: the app icon shown on the public Google Play page for this package name.
fn play_icon(pkg: &str) -> Result<Vec<u8>, String> {
    let page = fetch_url(&format!("https://play.google.com/store/apps/details?id={pkg}&hl=en&gl=US"))?;
    let html = String::from_utf8_lossy(&page).into_owned();
    let host = "https://play-lh.googleusercontent.com/";
    let start = html.find("property=\"og:image\" content=\"").map(|i| i + "property=\"og:image\" content=\"".len())
        .filter(|&i| html[i..].starts_with(host)).or_else(|| html.find(host)).ok_or_else(|| "Play sayfasında simge bulunamadı.".to_string())?;
    let url: String = html[start..].chars().take_while(|c| *c != '"' && *c != '\'' && *c != ')' && !c.is_whitespace()).collect();
    let base = url.split('=').next().unwrap_or("");
    if !base.starts_with(host) { return Err("Beklenmeyen simge adresi.".into()); }
    let bytes = fetch_url(&format!("{base}=s128"))?;
    if img_kind(&bytes).is_none() { return Err("Simge çözülemedi.".into()); }
    Ok(bytes)
}

fn icon_cache_dir() -> Option<PathBuf> { home_dir().map(|h| h.join(".cache/linux-flasher/icons")) }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IconReq { package: String, apk_path: String }

fn cache_read(dir: &Option<PathBuf>, pkg: &str) -> Option<IconOut> {
    let d = dir.as_ref()?;
    let bytes = fs::read(d.join(format!("{pkg}.bin"))).ok()?;
    img_kind(&bytes)?;
    let meta: Value = fs::read(d.join(format!("{pkg}.json"))).ok().and_then(|m| serde_json::from_slice(&m).ok()).unwrap_or(Value::Null);
    Some(IconOut { bytes, bg: meta["bg"].as_str().map(|s| s.to_string()), adaptive: meta["adaptive"].as_bool().unwrap_or(false) })
}
fn cache_write(dir: &Option<PathBuf>, pkg: &str, o: &IconOut) {
    if let Some(d) = dir {
        let _ = fs::write(d.join(format!("{pkg}.bin")), &o.bytes);
        let _ = fs::write(d.join(format!("{pkg}.json")), json!({"bg": o.bg, "adaptive": o.adaptive}).to_string());
    }
}

/// Resolves launcher icons and streams them to the UI as `app-icon` events ({package, icon, bg, adaptive}).
#[tauri::command]
pub async fn app_icons(app: AppHandle, serial: String, items: Vec<IconReq>, online: bool) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !ok_serial(&serial) { return Err("Geçersiz seri numarası.".to_string()); }
        let adbp = adb()?;
        let cache = icon_cache_dir();
        if let Some(c) = &cache { let _ = fs::create_dir_all(c); }
        let mut n = 0usize;
        for it in items {
            if BUSY.load(Ordering::SeqCst) { break; } // never compete with a running backup/restore
            if !ok_pkg(&it.package) { continue; }
            let icon = cache_read(&cache, &it.package).or_else(|| {
                let found = fetch_icon(&adbp, &serial, &it.apk_path).ok()
                    .or_else(|| if online { play_icon(&it.package).ok().map(|bytes| IconOut { bytes, bg: None, adaptive: false }) } else { None });
                if let Some(o) = &found { cache_write(&cache, &it.package, o); }
                found
            });
            let uri = icon.as_ref().and_then(|o| data_uri(&o.bytes));
            if uri.is_some() { n += 1; }
            let _ = app.emit("app-icon", json!({"package": it.package, "icon": uri, "bg": icon.as_ref().and_then(|o| o.bg.clone()), "adaptive": icon.as_ref().map(|o| o.adaptive).unwrap_or(false)}));
        }
        Ok(n)
    }).await.map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- codecs
struct Codec { ext: &'static str, comp: Option<(PathBuf, Vec<&'static str>)> }

fn pick_codec(pref: &str) -> Result<Codec, String> {
    let zstd = find_tool("zstd");
    let gzip = find_tool("gzip");
    match pref {
        "none" => Ok(Codec { ext: "tar", comp: None }),
        "gzip" => gzip.map(|g| Codec { ext: "tar.gz", comp: Some((g, vec!["-c", "-6"])) }).ok_or_else(|| "gzip bulunamadı.".to_string()),
        _ => {
            if let Some(z) = zstd { return Ok(Codec { ext: "tar.zst", comp: Some((z, vec!["-T0", "-3", "-q", "-c"])) }); }
            gzip.map(|g| Codec { ext: "tar.gz", comp: Some((g, vec!["-c", "-6"])) })
                .ok_or_else(|| "Sıkıştırma aracı bulunamadı (zstd veya gzip gerekli).".to_string())
        }
    }
}

fn codec_for_file(name: &str) -> Result<Option<(PathBuf, Vec<&'static str>)>, String> {
    if name.ends_with(".tar.zst") { Ok(Some((find_tool("zstd").ok_or_else(|| "zstd bulunamadı; bu yedek zstd ile sıkıştırılmış.".to_string())?, vec!["-dc", "-q"]))) }
    else if name.ends_with(".tar.gz") { Ok(Some((find_tool("gzip").ok_or_else(|| "gzip bulunamadı.".to_string())?, vec!["-dc"]))) }
    else if name.ends_with(".tar") { Ok(None) }
    else { Err(format!("Bilinmeyen arşiv türü: {name}")) }
}

/// adb exec-out (remote tar -c) -> optional compressor -> file. Returns file size.
fn stream_backup(adbp: &Path, serial: &str, remote: &str, out: &Path, codec: &Codec) -> Result<u64, String> {
    let file = fs::File::create(out).map_err(|e| format!("Dosya oluşturulamadı: {e}"))?;
    let mut a = Command::new(adbp);
    a.args(["-s", serial, "exec-out", remote]).stdin(Stdio::null()).stderr(Stdio::null());
    match &codec.comp {
        None => {
            let st = a.stdout(Stdio::from(file)).status().map_err(|e| format!("adb başlatılamadı: {e}"))?;
            if !st.success() { return Err("adb exec-out başarısız.".into()); }
        }
        Some((bin, args)) => {
            let mut ac = a.stdout(Stdio::piped()).spawn().map_err(|e| format!("adb başlatılamadı: {e}"))?;
            let pipe = ac.stdout.take().ok_or_else(|| "adb çıktısı alınamadı.".to_string())?;
            let mut c = Command::new(bin).args(args).stdin(Stdio::from(pipe)).stdout(Stdio::from(file)).stderr(Stdio::null())
                .spawn().map_err(|e| format!("Sıkıştırıcı başlatılamadı: {e}"))?;
            let cs = c.wait().map_err(|e| e.to_string())?;
            let _ = ac.wait();
            if !cs.success() { return Err("Sıkıştırma başarısız.".into()); }
        }
    }
    Ok(fs::metadata(out).map(|m| m.len()).unwrap_or(0))
}

/// Entry names of an archive, listed with host tar through the matching decompressor.
fn archive_entries(path: &Path) -> Result<Vec<String>, String> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let tar = find_tool("tar").ok_or_else(|| "Bilgisayarda tar bulunamadı.".to_string())?;
    let mut t = Command::new(tar);
    t.arg("-tf").arg("-").stdout(Stdio::piped()).stderr(Stdio::null());
    let mut dec_child = None;
    match codec_for_file(name)? {
        None => { t.stdin(Stdio::from(fs::File::open(path).map_err(|e| e.to_string())?)); }
        Some((bin, args)) => {
            let mut d = Command::new(bin).args(args).arg(path).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
            t.stdin(Stdio::from(d.stdout.take().ok_or_else(|| "Çıktı alınamadı.".to_string())?));
            dec_child = Some(d);
        }
    }
    let out = t.output().map_err(|e| format!("Arşiv doğrulanamadı: {e}"))?;
    if let Some(mut d) = dec_child { let _ = d.wait(); }
    if !out.status.success() { return Err("Arşiv bozuk görünüyor (tar -t başarısız).".into()); }
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect())
}

/// Returns the entry count; empty or corrupt archives are errors.
fn verify_archive(path: &Path) -> Result<usize, String> {
    let n = archive_entries(path)?.len();
    if n == 0 { return Err("Arşiv boş.".into()); }
    Ok(n)
}

/// Refuses archives with absolute paths, `..` components or members outside the expected location,
/// so a tampered backup cannot write elsewhere on the phone (as root) or on this computer.
fn entries_safe(names: &[String], allowed: impl Fn(&str) -> bool) -> Result<(), String> {
    for n in names {
        let t = n.trim_start_matches("./");
        if t.starts_with('/') || t.split('/').any(|c| c == "..") || !allowed(t) { return Err(format!("Arşivde güvensiz yol var: {n}")); }
    }
    Ok(())
}

fn package_uid(serial: &str, user: u32, pkg: &str) -> Option<String> {
    let (_, out) = adb_shell(serial, &format!("pm list packages -U --user {user} {pkg}"), 20).ok()?;
    out.lines().find_map(|l| {
        let mut it = l.trim().strip_prefix("package:")?.split_whitespace();
        if it.next()? != pkg { return None; }
        it.find_map(|t| t.strip_prefix("uid:")).filter(|u| !u.is_empty() && u.chars().all(|c| c.is_ascii_digit())).map(|u| u.to_string())
    })
}

// ---------------------------------------------------------------- layout
const KEYS: [&str; 5] = ["user", "userDe", "data", "obb", "media"];

fn comp_base(key: &str, user: u32) -> Option<String> {
    Some(match key {
        "user" => format!("/data/user/{user}"),
        "userDe" => format!("/data/user_de/{user}"),
        "data" => format!("/data/media/{user}/Android/data"),
        "obb" => format!("/data/media/{user}/Android/obb"),
        "media" => format!("/data/media/{user}/Android/media"),
        _ => return None,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataOpts {
    #[serde(default)] apk: bool, #[serde(default)] user: bool, #[serde(default)] user_de: bool,
    #[serde(default)] data: bool, #[serde(default)] obb: bool, #[serde(default)] media: bool,
    #[serde(default)] permissions: bool, #[serde(default)] clean: bool,
    #[serde(default)] appops: bool, #[serde(default)] online_icons: bool,
    #[serde(default)] compression: String,
}
impl DataOpts {
    fn on(&self, key: &str) -> bool { match key { "user" => self.user, "userDe" => self.user_de, "data" => self.data, "obb" => self.obb, "media" => self.media, _ => false } }
}

fn parse_perms(dump: &str, user: u32) -> Vec<String> {
    let (mut v, mut inr, mut cur) = (Vec::new(), false, 0u32);
    for l in dump.lines() {
        let t = l.trim();
        if let Some(r) = t.strip_prefix("User ") { if let Some(n) = r.split(':').next().and_then(|n| n.trim().parse::<u32>().ok()) { cur = n; inr = false; } }
        if t.starts_with("runtime permissions:") { inr = true; continue; }
        if inr {
            if let Some((name, rest)) = t.split_once(": granted=") { if cur == user && rest.starts_with("true") { v.push(name.to_string()); } }
            else { inr = false; }
        }
    }
    v.sort(); v.dedup(); v
}

fn field(dump: &str, key: &str) -> String {
    dump.lines().find_map(|l| l.trim().strip_prefix(key).map(|r| r.split_whitespace().next().unwrap_or("").to_string())).unwrap_or_default()
}

fn parse_appops(out: &str) -> Vec<String> {
    out.lines().filter_map(|l| {
        let (name, rest) = l.trim().split_once(':')?;
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') { return None; }
        let mode = rest.trim().split(|c: char| c == ';' || c.is_whitespace()).next()?;
        if !["allow", "ignore", "deny", "foreground"].contains(&mode) { return None; }
        Some(format!("{name}:{mode}"))
    }).collect()
}

fn ok_rel(s: &str) -> bool { !s.is_empty() && s.len() <= 200 && !s.starts_with('/') && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')) }

#[tauri::command]
pub fn default_backup_dir() -> String {
    home_dir().map(|h| h.join("LinuxFlasher-Backups").to_string_lossy().into_owned()).unwrap_or_else(|| "LinuxFlasher-Backups".into())
}

#[tauri::command]
pub async fn choose_dir(title: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(rfd::FileDialog::new().set_title(title.as_str()).pick_folder().map(|p| p.to_string_lossy().into_owned())))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn backup_cancel() { CANCEL.store(true, Ordering::SeqCst); }

fn timestamp() -> String {
    Command::new("date").arg("+%Y%m%d-%H%M%S").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
        .unwrap_or_else(|| "backup".into())
}

// ---------------------------------------------------------------- backup
#[tauri::command]
pub async fn backup_apps(app: AppHandle, serial: String, packages: Vec<String>, folders: Vec<String>, options: DataOpts, dest: String, user_id: u32) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || backup_blocking(&app, &serial, packages, folders, options, &dest, user_id)).await.map_err(|e| e.to_string())?
}

fn backup_blocking(app: &AppHandle, serial: &str, pkgs: Vec<String>, folders: Vec<String>, o: DataOpts, dest: &str, user: u32) -> Result<Value, String> {
    let _guard = BusyGuard::acquire()?;
    if pkgs.is_empty() && folders.is_empty() { return Err("Yedeklenecek uygulama seçilmedi.".into()); }
    if !pkgs.is_empty() && !(o.apk || KEYS.iter().any(|k| o.on(k))) { return Err("En az bir veri türü seçmelisin.".into()); }
    if let Some(bad) = folders.iter().find(|f| !ok_rel(f)) { return Err(format!("Geçersiz klasör yolu: {bad}")); }
    if let Some(bad) = pkgs.iter().find(|p| !ok_pkg(p)) { return Err(format!("Geçersiz paket adı: {bad}")); }
    // Mandatory gate: root through adb shell must be confirmed right before every backup.
    let root = root_check_blocking(serial)?;
    if !root.ok { return Err(format!("ADB shell root erişimi yok: {}", root.detail)); }
    let adbp = adb()?;
    let codec = pick_codec(&o.compression)?;
    let dev = list_devices_blocking()?.into_iter().find(|d| d.serial == serial);
    let model = dev.as_ref().map(|d| d.product.clone()).filter(|s| s != "unknown" && !s.is_empty()).unwrap_or_else(|| "android".into());
    let safe_model: String = model.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let root_dir = PathBuf::from(dest).join(format!("LinuxFlasher_{safe_model}_{}", timestamp()));
    fs::create_dir_all(&root_dir).map_err(|e| format!("Yedek klasörü oluşturulamadı: {e}"))?;
    emit_log(app, &format!("Yedek klasörü: {}", root_dir.display()), "info");
    let steps_per = 1 + KEYS.iter().filter(|k| o.on(k)).count() + usize::from(o.apk);
    let total = pkgs.len() + folders.len();
    let (mut apps_json, mut failed) = (Vec::new(), 0usize);
    for (i, pkg) in pkgs.iter().enumerate() {
        if CANCEL.load(Ordering::SeqCst) { emit_log(app, "Yedekleme kullanıcı tarafından iptal edildi.", "error"); break; }
        let base_pct = (i * 100 / total) as u32;
        emit_progress(app, "backup", pkg, "start", i + 1, total, base_pct, "running");
        emit_log(app, &format!("Yedekleniyor ({}/{}): {pkg}", i + 1, total), "info");
        match backup_pkg(app, &adbp, serial, &root.method, &codec, &o, user, pkg, &root_dir, i, total, steps_per) {
            Ok(meta) => { apps_json.push(meta); emit_progress(app, "backup", pkg, "done", i + 1, total, ((i + 1) * 100 / total) as u32, "done"); }
            Err(e) => { failed += 1; emit_log(app, &format!("{pkg}: {e}"), "error"); emit_progress(app, "backup", pkg, "failed", i + 1, total, ((i + 1) * 100 / total) as u32, "failed"); }
        }
    }
    let mut folders_json = Vec::new();
    for (j, rel) in folders.iter().enumerate() {
        if CANCEL.load(Ordering::SeqCst) { emit_log(app, "Yedekleme kullanıcı tarafından iptal edildi.", "error"); break; }
        let idx = pkgs.len() + j + 1;
        emit_progress(app, "backup", &format!("folder:{rel}"), "start", idx, total, ((idx - 1) * 100 / total) as u32, "running");
        match backup_folder(app, &adbp, serial, &root.method, &codec, user, rel, &root_dir) {
            Ok(meta) => { folders_json.push(meta); emit_progress(app, "backup", &format!("folder:{rel}"), "done", idx, total, (idx * 100 / total) as u32, "done"); }
            Err(e) => { failed += 1; emit_log(app, &format!("{rel}: {e}"), "error"); emit_progress(app, "backup", &format!("folder:{rel}"), "failed", idx, total, (idx * 100 / total) as u32, "failed"); }
        }
    }
    let manifest = json!({
        "schema": "linux-flasher-backup.v1",
        "createdAt": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "device": {"serial": serial, "product": model, "android": root.android, "sdk": root.sdk},
        "userId": user, "compression": codec.ext, "apps": apps_json, "folders": folders_json,
    });
    fs::write(root_dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?).map_err(|e| format!("Manifest yazılamadı: {e}"))?;
    let done = manifest["apps"].as_array().map(|a| a.len()).unwrap_or(0) + manifest["folders"].as_array().map(|a| a.len()).unwrap_or(0);
    let _ = app.emit("backup-finished", json!({"mode": "backup", "ok": failed == 0, "done": done, "failed": failed, "path": root_dir.to_string_lossy()}));
    Ok(json!({"path": root_dir.to_string_lossy(), "done": done, "failed": failed}))
}

fn backup_pkg(app: &AppHandle, adbp: &Path, serial: &str, method: &str, codec: &Codec, o: &DataOpts, user: u32, pkg: &str,
              root_dir: &Path, idx: usize, total: usize, steps_per: usize) -> Result<Value, String> {
    let pdir = root_dir.join(pkg);
    fs::create_dir_all(&pdir).map_err(|e| e.to_string())?;
    let (_, pm) = adb_shell(serial, &format!("pm path --user {user} {pkg}"), 20)?;
    let apks: Vec<String> = pm.lines().filter_map(|l| l.trim().strip_prefix("package:").map(|s| s.to_string())).collect();
    if apks.is_empty() || apks.iter().any(|a| !ok_remote_path(a)) { return Err("Paket bu kullanıcıda kurulu değil veya yolu geçersiz.".into()); }
    let dump = adb_shell(serial, &format!("dumpsys package {pkg}"), 40).map(|r| r.1).unwrap_or_default();
    let (version_name, version_code, uid) = (field(&dump, "versionName="), field(&dump, "versionCode="), field(&dump, "userId="));
    let perms = if o.permissions { parse_perms(&dump, user) } else { vec![] };
    // Stop the app so files are not changing while we archive them.
    let _ = adb_shell(serial, &rooted(method, &format!("am force-stop {pkg}"))?, 15);
    // The app icon is stored with the backup so the restore list can show it offline.
    let found = fetch_icon(adbp, serial, &apks[0]).ok()
        .or_else(|| if o.online_icons { play_icon(pkg).ok().map(|bytes| IconOut { bytes, bg: None, adaptive: false }) } else { None });
    let (icon_name, icon_bg, icon_adaptive) = match found {
        Some(ic) => match img_kind(&ic.bytes) {
            Some((_, ext)) => { let n = format!("icon.{ext}"); (fs::write(pdir.join(&n), &ic.bytes).ok().map(|_| n), ic.bg, ic.adaptive) }
            None => (None, None, false),
        },
        None => (None, None, false),
    };
    let appops = if o.appops { parse_appops(&adb_shell(serial, &format!("cmd appops get --user {user} {pkg}"), 25).map(|r| r.1).unwrap_or_default()) } else { vec![] };
    let (mut files, mut sizes, mut entries) = (serde_json::Map::new(), serde_json::Map::new(), serde_json::Map::new());
    let mut step = 0usize;
    let mut tick = |what: &str| {
        step += 1;
        let pct = ((idx * steps_per + step) * 100 / (total * steps_per)).min(99) as u32;
        emit_progress(app, "backup", pkg, what, idx + 1, total, pct, "running");
    };
    let mut archive = |key: &str, remote: String| -> Result<(), String> {
        let name = format!("{key}.{}", codec.ext);
        let out = pdir.join(&name);
        let size = stream_backup(adbp, serial, &rooted(method, &remote)?, &out, codec)?;
        let n = verify_archive(&out).map_err(|e| format!("{key}: {e}"))?;
        files.insert(key.into(), json!(name)); sizes.insert(key.into(), json!(size)); entries.insert(key.into(), json!(n));
        emit_log(app, &format!("{pkg}: {key} yedeklendi ({} KB, {n} öğe, doğrulandı)", size / 1024), "success");
        Ok(())
    };
    if o.apk {
        tick("apk");
        let dir = Path::new(&apks[0]).parent().and_then(|p| p.to_str()).unwrap_or("").to_string();
        let names: Vec<String> = apks.iter().filter_map(|a| Path::new(a).file_name().and_then(|n| n.to_str()).map(|s| s.to_string())).collect();
        archive("apk", format!("tar -cf - -C {dir} {} 2>/dev/null", names.join(" ")))?;
    }
    for key in KEYS {
        if !o.on(key) { continue; }
        tick(key);
        let base = comp_base(key, user).ok_or_else(|| "Bilinmeyen veri türü.".to_string())?;
        let (_, ex) = adb_shell(serial, &rooted(method, &format!("[ -d {base}/{pkg} ] && echo yes"))?, 15)?;
        if !ex.contains("yes") { emit_log(app, &format!("{pkg}: {key} dizini yok, atlandı."), "info"); continue; }
        archive(key, format!("tar -cpf - -C {base} --exclude={pkg}/cache --exclude={pkg}/code_cache {pkg} 2>/dev/null"))?;
    }
    Ok(json!({"package": pkg, "versionName": version_name, "versionCode": version_code, "uid": uid, "splitCount": apks.len(),
              "files": files, "sizes": sizes, "entries": entries, "permissions": perms, "appops": appops, "icon": icon_name, "iconBg": icon_bg, "iconAdaptive": icon_adaptive}))
}

fn backup_folder(app: &AppHandle, adbp: &Path, serial: &str, method: &str, codec: &Codec, user: u32, rel: &str, root_dir: &Path) -> Result<Value, String> {
    let base = format!("/data/media/{user}");
    let (_, ex) = adb_shell(serial, &rooted(method, &format!("[ -e {base}/{rel} ] && echo yes"))?, 15)?;
    if !ex.contains("yes") { return Err("Klasör cihazda yok.".into()); }
    let dir = root_dir.join("_folders");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = format!("{}.{}", rel.replace('/', "__"), codec.ext);
    let out = dir.join(&name);
    emit_log(app, &format!("Klasör yedekleniyor: {rel}"), "info");
    let size = stream_backup(adbp, serial, &rooted(method, &format!("tar -cpf - -C {base} {rel} 2>/dev/null"))?, &out, codec)?;
    let n = verify_archive(&out)?;
    emit_log(app, &format!("Klasör yedeklendi: {rel} ({} MB, {n} öğe, doğrulandı)", size / 1048576), "success");
    Ok(json!({"path": rel, "file": format!("_folders/{name}"), "size": size, "entries": n}))
}

fn restore_folder(app: &AppHandle, adbp: &Path, serial: &str, method: &str, user: u32, dir: &Path, entry: &Value) -> Result<(), String> {
    let rel = entry["path"].as_str().unwrap_or("");
    let file = entry["file"].as_str().unwrap_or("");
    let fname = file.strip_prefix("_folders/").unwrap_or("");
    if !ok_rel(rel) || !ok_file_name(fname) { return Err("Yedekteki klasör kaydı geçersiz.".into()); }
    let path = dir.join("_folders").join(fname);
    if !path.is_file() { return Err("Klasör arşivi bulunamadı.".into()); }
    let base = format!("/data/media/{user}");
    sh_root(serial, method, &format!("mkdir -p {base}"), 15)?;
    entries_safe(&archive_entries(&path)?, |t| t == rel || t.starts_with(&format!("{rel}/")))?;
    stream_restore(adbp, serial, &rooted(method, &format!("tar -xpf - -C {base}"))?, &path)?;
    let _ = sh_root(serial, method, &format!("restorecon -RF {base}/{rel}"), 300);
    emit_log(app, &format!("Klasör geri yüklendi: {rel}"), "success");
    Ok(())
}

// ---------------------------------------------------------------- list backups
#[tauri::command]
pub async fn list_backups(dir: String) -> Result<Vec<Value>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = PathBuf::from(&dir);
        let mut cands: Vec<PathBuf> = vec![root.clone()];
        if let Ok(rd) = fs::read_dir(&root) { for e in rd.flatten() { if e.path().is_dir() { cands.push(e.path()); } } }
        let mut out = Vec::new();
        for c in cands {
            let Ok(bytes) = fs::read(c.join("manifest.json")) else { continue };
            let Ok(mut m) = serde_json::from_slice::<Value>(&bytes) else { continue };
            if m["schema"] != "linux-flasher-backup.v1" { continue; }
            let size: u64 = m["apps"].as_array().map(|a| a.iter().flat_map(|x| x["sizes"].as_object().into_iter().flat_map(|s| s.values().filter_map(|v| v.as_u64()))).sum()).unwrap_or(0);
            if let Some(arr) = m["apps"].as_array_mut() {
                for a in arr.iter_mut() {
                    let (pkg, icon) = (a["package"].as_str().unwrap_or("").to_string(), a["icon"].as_str().unwrap_or("").to_string());
                    if ok_pkg(&pkg) && ok_file_name(&icon) {
                        if let Some(uri) = fs::read(c.join(&pkg).join(&icon)).ok().and_then(|bytes| data_uri(&bytes)) { a["iconData"] = json!(uri); }
                    }
                }
            }
            m["path"] = json!(c.to_string_lossy());
            m["name"] = json!(c.file_name().and_then(|n| n.to_str()).unwrap_or(""));
            m["totalSize"] = json!(size);
            out.push(m);
        }
        out.sort_by(|a, b| b["createdAt"].as_u64().cmp(&a["createdAt"].as_u64()));
        Ok(out)
    }).await.map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- restore
#[tauri::command]
pub async fn restore_apps(app: AppHandle, serial: String, backup: String, packages: Vec<String>, folders: Vec<String>, options: DataOpts, user_id: u32) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || restore_blocking(&app, &serial, &backup, packages, folders, options, user_id)).await.map_err(|e| e.to_string())?
}

fn restore_blocking(app: &AppHandle, serial: &str, backup: &str, pkgs: Vec<String>, folders: Vec<String>, o: DataOpts, user: u32) -> Result<Value, String> {
    let _guard = BusyGuard::acquire()?;
    if pkgs.is_empty() && folders.is_empty() { return Err("Geri yüklenecek uygulama seçilmedi.".into()); }
    if let Some(bad) = pkgs.iter().find(|p| !ok_pkg(p)) { return Err(format!("Geçersiz paket adı: {bad}")); }
    let root = root_check_blocking(serial)?;
    if !root.ok { return Err(format!("ADB shell root erişimi yok: {}", root.detail)); }
    let adbp = adb()?;
    let dir = PathBuf::from(backup);
    let manifest: Value = serde_json::from_slice(&fs::read(dir.join("manifest.json")).map_err(|e| format!("Manifest okunamadı: {e}"))?).map_err(|e| format!("Manifest bozuk: {e}"))?;
    let apps = manifest["apps"].as_array().cloned().unwrap_or_default();
    let total = pkgs.len() + folders.len();
    let (mut done, mut failed) = (0usize, 0usize);
    for (i, pkg) in pkgs.iter().enumerate() {
        if CANCEL.load(Ordering::SeqCst) { emit_log(app, "Geri yükleme kullanıcı tarafından iptal edildi.", "error"); break; }
        emit_progress(app, "restore", pkg, "start", i + 1, total, (i * 100 / total) as u32, "running");
        emit_log(app, &format!("Geri yükleniyor ({}/{}): {pkg}", i + 1, total), "info");
        let Some(entry) = apps.iter().find(|a| a["package"] == pkg.as_str()) else {
            failed += 1; emit_log(app, &format!("{pkg}: yedekte bulunamadı."), "error"); continue;
        };
        match restore_pkg(app, &adbp, serial, &root.method, &o, user, pkg, &dir, entry) {
            Ok(()) => { done += 1; emit_progress(app, "restore", pkg, "done", i + 1, total, ((i + 1) * 100 / total) as u32, "done"); }
            Err(e) => { failed += 1; emit_log(app, &format!("{pkg}: {e}"), "error"); emit_progress(app, "restore", pkg, "failed", i + 1, total, ((i + 1) * 100 / total) as u32, "failed"); }
        }
    }
    let folder_entries = manifest["folders"].as_array().cloned().unwrap_or_default();
    for (j, rel) in folders.iter().enumerate() {
        if CANCEL.load(Ordering::SeqCst) { break; }
        let idx = pkgs.len() + j + 1;
        emit_progress(app, "restore", &format!("folder:{rel}"), "start", idx, total, ((idx - 1) * 100 / total) as u32, "running");
        let res = folder_entries.iter().find(|f| f["path"] == rel.as_str()).ok_or_else(|| "yedekte bulunamadı.".to_string())
            .and_then(|entry| restore_folder(app, &adbp, serial, &root.method, user, &dir, entry));
        match res {
            Ok(()) => { done += 1; emit_progress(app, "restore", &format!("folder:{rel}"), "done", idx, total, (idx * 100 / total) as u32, "done"); }
            Err(e) => { failed += 1; emit_log(app, &format!("{rel}: {e}"), "error"); emit_progress(app, "restore", &format!("folder:{rel}"), "failed", idx, total, (idx * 100 / total) as u32, "failed"); }
        }
    }
    let _ = app.emit("backup-finished", json!({"mode": "restore", "ok": failed == 0, "done": done, "failed": failed, "path": backup}));
    Ok(json!({"done": done, "failed": failed}))
}

fn sh_root(serial: &str, method: &str, cmd: &str, secs: u64) -> Result<String, String> {
    let (ok, out) = adb_shell(serial, &rooted(method, cmd)?, secs)?;
    if !ok && !out.is_empty() { return Err(out); }
    Ok(out)
}

/// Pipes <archive> (through the decompressor) into `adb shell su -c "tar -x ..."`.
fn stream_restore(adbp: &Path, serial: &str, remote: &str, file: &Path) -> Result<String, String> {
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let mut a = Command::new(adbp).args(["-s", serial, "shell", remote]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("adb başlatılamadı: {e}"))?;
    let mut stdin = a.stdin.take().ok_or_else(|| "adb stdin açılamadı.".to_string())?;
    let (mut so, mut se) = (a.stdout.take(), a.stderr.take());
    let h1 = thread::spawn(move || { let mut b = Vec::new(); if let Some(r) = so.as_mut() { let _ = r.read_to_end(&mut b); } b });
    let h2 = thread::spawn(move || { let mut b = Vec::new(); if let Some(r) = se.as_mut() { let _ = r.read_to_end(&mut b); } b });
    let copy_res = match codec_for_file(name)? {
        None => fs::File::open(file).and_then(|mut f| std::io::copy(&mut f, &mut stdin)).map(|_| ()).map_err(|e| e.to_string()),
        Some((bin, args)) => {
            let mut d = Command::new(bin).args(args).arg(file).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
            let mut dout = d.stdout.take().ok_or_else(|| "Çıktı alınamadı.".to_string())?;
            let r = std::io::copy(&mut dout, &mut stdin).map(|_| ()).map_err(|e| e.to_string());
            let st = d.wait().map_err(|e| e.to_string())?;
            if r.is_ok() && !st.success() { Err("Açma (decompress) başarısız.".to_string()) } else { r }
        }
    };
    drop(stdin);
    let st = a.wait().map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&h1.join().unwrap_or_default()), String::from_utf8_lossy(&h2.join().unwrap_or_default()));
    copy_res?;
    if !st.success() { return Err(format!("tar açma başarısız: {}", text.trim())); }
    Ok(text.trim().to_string())
}

fn restore_pkg(app: &AppHandle, adbp: &Path, serial: &str, method: &str, o: &DataOpts, user: u32, pkg: &str, dir: &Path, entry: &Value) -> Result<(), String> {
    let files = entry["files"].as_object().cloned().unwrap_or_default();
    let file_of = |key: &str| -> Option<PathBuf> {
        let n = files.get(key)?.as_str()?;
        if !ok_file_name(n) { return None; }
        let p = dir.join(pkg).join(n);
        p.is_file().then_some(p)
    };
    // 1) install APK(s)
    if o.apk {
        let f = file_of("apk").ok_or_else(|| "Yedekte APK arşivi yok.".to_string())?;
        let tmp = std::env::temp_dir().join(format!("lf-restore-{}-{pkg}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        entries_safe(&archive_entries(&f)?, |t| !t.contains('/') && t.ends_with(".apk"))?;
        let tar = find_tool("tar").ok_or_else(|| "Bilgisayarda tar bulunamadı.".to_string())?;
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let mut t = Command::new(tar);
        t.arg("-xf").arg("-").arg("-C").arg(&tmp).stdout(Stdio::null()).stderr(Stdio::null());
        let mut dec = None;
        match codec_for_file(&name)? {
            None => { t.stdin(Stdio::from(fs::File::open(&f).map_err(|e| e.to_string())?)); }
            Some((bin, args)) => {
                let mut d = Command::new(bin).args(args).arg(&f).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
                t.stdin(Stdio::from(d.stdout.take().ok_or_else(|| "Çıktı alınamadı.".to_string())?));
                dec = Some(d);
            }
        }
        let st = t.status().map_err(|e| e.to_string())?;
        if let Some(mut d) = dec { let _ = d.wait(); }
        if !st.success() { return Err("APK arşivi açılamadı.".into()); }
        let mut apks: Vec<PathBuf> = fs::read_dir(&tmp).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|x| x.to_str()) == Some("apk")).collect();
        apks.sort();
        if apks.is_empty() { return Err("Arşivde APK yok.".into()); }
        let mut c = Command::new(adbp);
        c.args(["-s", serial, "install-multiple", "-r", "-t"]);
        if user != 0 { c.arg("--user").arg(user.to_string()); }
        let out = c.args(&apks).output().map_err(|e| format!("adb install başlatılamadı: {e}"))?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let _ = fs::remove_dir_all(&tmp);
        if !text.contains("Success") { return Err(format!("APK kurulamadı: {}", text.trim())); }
        emit_log(app, &format!("{pkg}: APK kuruldu ({} dosya).", apks.len()), "success");
    }
    // 2) the app must exist before its data can be restored
    let (_, pm) = adb_shell(serial, &format!("pm path --user {user} {pkg}"), 20)?;
    if !pm.contains("package:") { return Err("Uygulama kurulu değil; veriyi geri yüklemek için APK da seçilmeli.".into()); }
    let uid_s = package_uid(serial, user, pkg)
        .or_else(|| sh_root(serial, method, &format!("stat -c %u /data/user/{user}/{pkg}"), 15).ok().map(|s| s.trim().to_string()).filter(|s| s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty()))
        .or_else(|| Some(entry["uid"].as_str().unwrap_or("").to_string()).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())))
        .ok_or_else(|| "Uygulamanın uid değeri belirlenemedi.".to_string())?;
    let _ = sh_root(serial, method, &format!("am force-stop {pkg}"), 15);
    // 3) data
    for key in KEYS {
        if !o.on(key) { continue; }
        let Some(f) = file_of(key) else { continue };
        let base = comp_base(key, user).ok_or_else(|| "Bilinmeyen veri türü.".to_string())?;
        let target = format!("{base}/{pkg}");
        // external dirs: keep the group of the parent (ext_data_rw / ext_obb_rw / media_rw); internal: gid == uid
        let gid = if key == "user" || key == "userDe" { uid_s.clone() } else {
            sh_root(serial, method, &format!("stat -c %g {base}"), 15).map(|s| s.trim().to_string()).ok().filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).unwrap_or_else(|| uid_s.clone())
        };
        sh_root(serial, method, &format!("mkdir -p {base}"), 15)?;
        if o.clean { let _ = sh_root(serial, method, &format!("[ -d {target} ] && cd {target} && rm -rf ./* ./.[!.]* ./..?*; true"), 60); }
        entries_safe(&archive_entries(&f)?, |t| t == pkg || t.starts_with(&format!("{pkg}/"))).map_err(|e| format!("{key}: {e}"))?;
        stream_restore(adbp, serial, &rooted(method, &format!("tar -xpf - -C {base}"))?, &f).map_err(|e| format!("{key}: {e}"))?;
        sh_root(serial, method, &format!("chown -R {uid_s}:{gid} {target}"), 120)?;
        let _ = sh_root(serial, method, &format!("restorecon -RF {target}"), 120);
        emit_log(app, &format!("{pkg}: {key} geri yüklendi (uid {uid_s}, gid {gid}, SELinux yenilendi)."), "success");
    }
    // 4) runtime permissions and AppOps (batched: one adb call per ~20 commands)
    let strs = |key: &str| -> Vec<String> { entry[key].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_default() };
    if o.permissions {
        let perms: Vec<String> = strs("permissions").into_iter().filter(|p| p.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')).collect();
        let mut failed_grants = 0usize;
        for chunk in perms.chunks(20) {
            let cmd = chunk.iter().map(|p| format!("pm grant --user {user} {pkg} {p}")).collect::<Vec<_>>().join("; ");
            if let Ok((_, out)) = adb_shell(serial, &cmd, 90) { failed_grants += out.lines().filter(|l| l.contains("Exception") || l.contains("Error") || l.contains("not a changeable")).count(); }
        }
        emit_log(app, &format!("{pkg}: {}/{} izin verildi.", perms.len().saturating_sub(failed_grants), perms.len()), "info");
    }
    if o.appops {
        let ops: Vec<(String, String)> = strs("appops").into_iter().filter_map(|s| {
            let (n, m) = s.split_once(':')?;
            (n.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') && ["allow", "ignore", "deny", "foreground"].contains(&m)).then(|| (n.to_string(), m.to_string()))
        }).collect();
        for chunk in ops.chunks(20) {
            let cmd = chunk.iter().map(|(n, m)| format!("cmd appops set --user {user} {pkg} {n} {m}")).collect::<Vec<_>>().join("; ");
            let _ = adb_shell(serial, &cmd, 90);
        }
        if !ops.is_empty() { emit_log(app, &format!("{pkg}: {} AppOps ayarı geri yüklendi.", ops.len()), "info"); }
    }
    Ok(())
}
