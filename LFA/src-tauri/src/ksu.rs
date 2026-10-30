//! KernelSU boot-image patching. ksud and the matching `<kmi>_kernelsu.ko` are downloaded from a GitHub release,
//! pushed to the TARGET phone over ADB and executed there (Android does not let this app execute downloaded files).
//! The patched image is pulled back and flashed through the fastboot session.
use crate::adb::Adb;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs::{self, File}, io::{Read, Write}, path::{Path, PathBuf}, time::Duration};
use tauri::{AppHandle, Emitter, Manager};

fn e<E: std::fmt::Display>(x: E) -> String { x.to_string() }
fn ok_repo(s: &str) -> bool {
    let p: Vec<&str> = s.split('/').collect();
    p.len() == 2 && p.iter().all(|x| !x.is_empty() && x.len() <= 100 && x.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
}
fn ok_tag(s: &str) -> bool { !s.is_empty() && s.len() <= 80 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+')) }
fn ok_name(s: &str) -> bool { !s.is_empty() && s.len() <= 120 && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+')) }
fn ok_kmi(s: &str) -> bool {
    match s.split_once('-') {
        Some((a, k)) => a.len() > 7 && a.starts_with("android") && a[7..].chars().all(|c| c.is_ascii_digit()) && !k.is_empty() && k.chars().all(|c| c.is_ascii_digit() || c == '.'),
        None => false,
    }
}
fn parse_kmi(s: &str) -> Option<String> {
    let i = s.find("-android")?;
    let av: String = s[i + 1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if !av.starts_with("android") { return None; }
    let rev: String = s[..i].chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let ver: String = rev.chars().rev().collect();
    let mut p = ver.split('.');
    let kmi = format!("{av}-{}.{}", p.next()?, p.next()?);
    ok_kmi(&kmi).then_some(kmi)
}

struct Asset { name: String, url: String, size: u64, digest: Option<String> }
fn assets_of(rel: &Value) -> Vec<Asset> {
    rel["assets"].as_array().map(|a| a.iter().filter_map(|x| Some(Asset {
        name: x["name"].as_str()?.to_string(), url: x["browser_download_url"].as_str()?.to_string(),
        size: x["size"].as_u64().unwrap_or(0), digest: x["digest"].as_str().map(String::from),
    })).collect()).unwrap_or_default()
}
fn kmi_of_asset(n: &str) -> Option<String> { let k = n.strip_suffix("_kernelsu.ko")?; ok_kmi(k).then(|| k.to_string()) }
fn pick_manager(assets: &[Asset], spoofed: bool) -> Option<&Asset> {
    let apks: Vec<&Asset> = assets.iter().filter(|a| { let n = a.name.to_lowercase(); n.ends_with(".apk") && !n.contains("debug") }).collect();
    apks.iter().find(|a| a.name.to_lowercase().contains("spoof") == spoofed).copied().or_else(|| apks.first().copied())
}

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let r = ureq::get(url).set("User-Agent", "oplus-flasher").set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(30)).call().map_err(|x| format!("Ağ hatası: {x}"))?;
    let mut b = vec![];
    r.into_reader().take(8 << 20).read_to_end(&mut b).map_err(e)?;
    Ok(b)
}
fn github_json(url: &str) -> Result<Value, String> { serde_json::from_slice(&http_get(url)?).map_err(|x| format!("GitHub yanıtı okunamadı: {x}")) }
fn download(url: &str, dest: &Path) -> Result<(), String> {
    let r = ureq::get(url).set("User-Agent", "oplus-flasher").timeout(Duration::from_secs(600)).call().map_err(|x| format!("İndirme hatası: {x}"))?;
    let tmp = dest.with_extension("part");
    let mut f = File::create(&tmp).map_err(e)?;
    std::io::copy(&mut r.into_reader(), &mut f).map_err(e)?;
    drop(f);
    fs::rename(&tmp, dest).map_err(e)
}
fn verify_digest(p: &Path, digest: &Option<String>) -> Result<(), String> {
    let Some(want) = digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) else { return Ok(()) };
    let mut f = File::open(p).map_err(e)?;
    let (mut h, mut b) = (Sha256::new(), vec![0u8; 1 << 16]);
    loop { let n = f.read(&mut b).map_err(e)?; if n == 0 { break; } h.update(&b[..n]); }
    let got: String = h.finalize().iter().map(|x| format!("{x:02x}")).collect();
    if got.eq_ignore_ascii_case(want) { Ok(()) } else { let _ = fs::remove_file(p); Err("İndirilen dosyanın SHA-256 değeri uyuşmuyor; dosya silindi.".into()) }
}
fn cache_dir(app: &AppHandle, repo: &str, tag: &str) -> Result<PathBuf, String> {
    let d = app.path().app_cache_dir().map_err(e)?.join("ksu").join(repo.replace('/', "__")).join(tag);
    fs::create_dir_all(&d).map_err(e)?;
    Ok(d)
}
fn ensure_asset(app: &AppHandle, repo: &str, dir: &Path, a: &Asset) -> Result<PathBuf, String> {
    if !ok_name(&a.name) { return Err("Geçersiz dosya adı.".into()); }
    if !a.url.starts_with(&format!("https://github.com/{repo}/releases/download/")) { return Err(format!("Beklenmeyen indirme adresi: {}", a.url)); }
    let p = dir.join(&a.name);
    if fs::metadata(&p).map(|m| m.len() == a.size && a.size > 0).unwrap_or(false) && verify_digest(&p, &a.digest).is_ok() { return Ok(p); }
    let _ = app.emit("ksu-log", format!("{} indiriliyor…", a.name));
    download(&a.url, &p)?;
    verify_digest(&p, &a.digest)?;
    Ok(p)
}
#[tauri::command]
pub async fn ksu_releases(repo: String) -> Result<Vec<Value>, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<Value>, String> {
        if !ok_repo(&repo) { return Err("Geçersiz depo adı (sahip/depo).".into()); }
        let list = github_json(&format!("https://api.github.com/repos/{repo}/releases?per_page=8"))?;
        let mut out = vec![];
        for rel in list.as_array().ok_or("Sürüm listesi alınamadı.")?.iter().filter(|r| r["draft"] != true) {
            let assets = assets_of(rel);
            let mut kmis: Vec<String> = assets.iter().filter_map(|a| kmi_of_asset(&a.name)).collect();
            kmis.sort(); kmis.dedup();
            out.push(json!({"tag": rel["tag_name"], "name": rel["name"], "prerelease": rel["prerelease"], "kmis": kmis, "apk": pick_manager(&assets, false).is_some()}));
        }
        Ok(out)
    }).await.map_err(e)?
}

#[tauri::command]
pub async fn ksu_scan(folder: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let dir = PathBuf::from(&folder);
        if !dir.is_dir() { return Err("Klasör bulunamadı.".into()); }
        let mut images = vec![];
        for n in ["init_boot.img", "boot.img", "vendor_boot.img", "vbmeta.img"] {
            let p = dir.join(n);
            if let Ok(m) = fs::metadata(&p) { if m.is_file() { images.push(json!({"name": n, "size": m.len(), "path": p.to_string_lossy()})); } }
        }
        let has = |n: &str| images.iter().any(|i| i["name"] == n);
        let rec = if has("init_boot.img") { Some("init_boot") } else if has("boot.img") { Some("boot") } else { None };
        Ok(json!({"folder": folder, "images": images, "recommended": rec}))
    }).await.map_err(e)?
}

/// `uname -r` of the ADB-connected device first, otherwise the kernel string inside an uncompressed boot.img.
pub fn detect_kmi(uname: Option<String>, folder: &str) -> Value {
    if let Some(u) = uname { if let Some(k) = parse_kmi(&u) { return json!({"kmi": k, "source": "cihaz", "kernel": u.trim()}); } }
    if let Ok(b) = fs::read(PathBuf::from(folder).join("boot.img")) {
        let needle = b"Linux version ";
        if let Some(pos) = b.windows(needle.len()).position(|w| w == needle) {
            let text = String::from_utf8_lossy(&b[pos..(pos + 160).min(b.len())]).to_string();
            let line = text.lines().next().unwrap_or("").to_string();
            if let Some(k) = parse_kmi(&line) { return json!({"kmi": k, "source": "boot.img", "kernel": line}); }
        }
    }
    json!({"kmi": null, "source": "yok", "kernel": ""})
}

const KDIR: &str = "/data/local/tmp/oplus-ksu";

fn pick_ksud(assets: &[Asset]) -> Option<&Asset> {
    assets.iter().find(|a| { let n = a.name.to_lowercase(); n.contains("ksud") && n.contains("aarch64") && !n.ends_with(".sha256") && !n.ends_with(".sig") && !n.ends_with(".txt") })
}

fn push(a: &mut Adb, local: &Path, remote: &str) -> Result<(), String> {
    let size = fs::metadata(local).map_err(e)?.len();
    let mut f = File::open(local).map_err(e)?;
    crate::backup::feed(a, &format!("exec:head -c {size} > {remote}"), &mut f)?;
    let got = a.exec(&format!("stat -c %s {remote}"))?.trim().parse::<u64>().unwrap_or(0);
    if got != size { return Err(format!("{remote}: eksik aktarım ({got}/{size})")); }
    Ok(())
}

/// ksud cannot be executed from this app (Android blocks it), so it is pushed to the TARGET phone and run there
/// from /data/local/tmp with the ADB shell; the patched image is pulled back afterwards.
fn device_patch(app: &AppHandle, a: &mut Adb, ksud: &Path, ko: Option<&Path>, img: &Path, outdir: &Path, partition: &str, kmi: &str, tag: &str, repo: &str) -> Result<Value, String> {
    let log = |m: &str| { let _ = app.emit("ksu-log", m.to_string()); };
    if !a.exec(&format!("rm -rf {KDIR}; mkdir -p {KDIR} && echo ok"))?.contains("ok") { return Err("Hedef cihazda çalışma klasörü oluşturulamadı".into()); }
    let abi = a.exec("getprop ro.product.cpu.abi")?;
    if !abi.contains("arm64") { return Err(format!("Hedef cihaz arm64 değil ({})", abi.trim())); }
    log("ksud hedef cihaza gönderiliyor…");
    push(a, ksud, &format!("{KDIR}/ksud"))?;
    a.exec(&format!("chmod 755 {KDIR}/ksud"))?;
    log("imaj hedef cihaza gönderiliyor…");
    push(a, img, &format!("{KDIR}/{partition}.img"))?;
    let ko_remote = format!("{KDIR}/{kmi}_kernelsu.ko");
    if let Some(k) = ko { push(a, k, &ko_remote)?; }
    let help = a.exec(&format!("cd {KDIR} && ./ksud boot-patch --help 2>&1"))?;
    let has = |f: &str| help.contains(f);
    let mut cmd = format!("cd {KDIR} && TMPDIR={KDIR} HOME={KDIR} ./ksud boot-patch {} {KDIR}/{partition}.img", if has("--boot") { "--boot" } else { "-b" });
    if has("--kmi") { cmd += &format!(" --kmi {kmi}"); }
    if ko.is_some() { if has("--lkm") { cmd += &format!(" --lkm {ko_remote}"); } else if has("--module") { cmd += &format!(" --module {ko_remote}"); } }
    cmd += " 2>&1; echo EXIT:$?";
    log(&format!("ksud boot-patch {partition}.img (KMI {kmi})"));
    let (mut buf, mut tail) = (String::new(), Vec::<String>::new());
    a.run(&format!("exec:{cmd}"), None, &mut |d| {
        buf.push_str(&String::from_utf8_lossy(d));
        while let Some(i) = buf.find('\n') {
            let line = buf[..i].trim_end().to_string();
            buf.drain(..=i);
            if !line.is_empty() { log(&line); tail.push(line); }
        }
    })?;
    if !buf.trim().is_empty() { tail.push(buf.trim().to_string()); }
    if tail.last().map(|l| l.trim()) != Some("EXIT:0") {
        return Err(format!("ksud boot-patch başarısız:\n{}", tail.iter().rev().take(8).rev().cloned().collect::<Vec<_>>().join("\n")));
    }
    let ls = a.exec(&format!("ls -t {KDIR}/*.img 2>/dev/null"))?;
    let out_remote = ls.lines().map(|l| l.trim()).find(|l| !l.is_empty() && !l.ends_with(&format!("/{partition}.img"))).ok_or("ksud çıktı dosyası bulunamadı")?.to_string();
    if !out_remote.starts_with(KDIR) || out_remote.contains("..") || !out_remote.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)) { return Err(format!("beklenmeyen çıktı yolu: {out_remote}")); }
    let want = a.exec(&format!("stat -c %s {out_remote}"))?.trim().parse::<u64>().unwrap_or(0);
    let fin = outdir.join(format!("{partition}_{}_{tag}_{kmi}.img", repo.replace('/', "_")));
    let mut f = File::create(&fin).map_err(e)?;
    let (mut werr, mut got) = (None, 0u64);
    log("yamalı imaj indiriliyor…");
    a.run(&format!("exec:cat {out_remote}"), None, &mut |d| { if let Err(x) = f.write_all(d) { werr = Some(x.to_string()); } got += d.len() as u64; })?;
    drop(f);
    if let Some(x) = werr { return Err(x); }
    if want == 0 || got != want { return Err(format!("imaj eksik indirildi ({got}/{want})")); }
    Ok(json!({"path": fin.to_string_lossy(), "partition": partition, "kmi": kmi, "tag": tag, "size": got}))
}

#[tauri::command]
pub async fn ksu_patch(app: AppHandle, repo: String, tag: String, folder: String, partition: String, kmi: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        if !ok_repo(&repo) || !ok_tag(&tag) { return Err("Geçersiz depo veya sürüm.".into()); }
        if !ok_kmi(&kmi) { return Err(format!("Geçersiz KMI: {kmi}")); }
        if partition != "init_boot" && partition != "boot" { return Err("Bölüm init_boot veya boot olmalı.".into()); }
        let img = PathBuf::from(&folder).join(format!("{partition}.img"));
        if !img.is_file() { return Err(format!("{partition}.img klasörde bulunamadı.")); }
        let rel = github_json(&format!("https://api.github.com/repos/{repo}/releases/tags/{tag}"))?;
        let assets = assets_of(&rel);
        let dir = cache_dir(&app, &repo, &tag)?;
        let ksud_asset = pick_ksud(&assets).ok_or_else(|| format!("Bu sürümde aarch64 ksud dosyası yok. Dosyalar: {}", assets.iter().map(|a| a.name.as_str()).take(12).collect::<Vec<_>>().join(", ")))?;
        let ksud = ensure_asset(&app, &repo, &dir, ksud_asset)?;
        let ko_name = format!("{kmi}_kernelsu.ko");
        let ko = match assets.iter().find(|a| a.name == ko_name) {
            Some(a) => Some(ensure_asset(&app, &repo, &dir, a)?),
            None => { let _ = app.emit("ksu-log", format!("{ko_name} sürümde yok; ksud içindeki gömülü modül denenecek.")); None }
        };
        let outdir = PathBuf::from(&folder).join("ksu-patched");
        fs::create_dir_all(&outdir).map_err(e)?;
        crate::with_adb(&app, |a| {
            let r = device_patch(&app, a, &ksud, ko.as_deref(), &img, &outdir, &partition, &kmi, &tag, &repo);
            let _ = a.exec(&format!("rm -rf {KDIR}"));
            r
        })
    }).await.map_err(e)?
}

/// fastboot's --disable-verity --disable-verification: set the AVB header flags (u32 BE at offset 120) to 3.
pub fn disable_verity(src: &Path, dst: &Path) -> Result<(), String> {
    let mut b = fs::read(src).map_err(e)?;
    if b.len() < 256 || &b[..4] != b"AVB0" { return Err("vbmeta.img geçerli değil".into()); }
    b[123] |= 3;
    fs::write(dst, &b).map_err(e)
}

pub fn fetch_manager(app: &AppHandle, repo: &str, tag: &str, spoofed: bool) -> Result<PathBuf, String> {
    if !ok_repo(repo) || !ok_tag(tag) { return Err("Geçersiz depo veya sürüm.".into()); }
    let rel = github_json(&format!("https://api.github.com/repos/{repo}/releases/tags/{tag}"))?;
    let assets = assets_of(&rel);
    let a = pick_manager(&assets, spoofed).ok_or("Bu sürümde yönetici APK’sı yok.")?;
    ensure_asset(app, repo, &cache_dir(app, repo, tag)?, a)
}

pub fn install_apk(a: &mut Adb, p: &Path) -> Result<String, String> {
    let size = fs::metadata(p).map_err(e)?.len();
    let o = a.exec(&format!("pm install-create -r -t -S {size}"))?;
    let sid: String = o.split('[').nth(1).and_then(|s| s.split(']').next()).unwrap_or("").to_string();
    if sid.is_empty() || !sid.chars().all(|c| c.is_ascii_digit()) { return Err(format!("kurulum oturumu açılamadı: {o}")); }
    let mut f = File::open(p).map_err(e)?;
    let mut out = vec![];
    a.run(&format!("exec:pm install-write -S {size} {sid} 0.apk -"), Some(&mut f), &mut |d| out.extend_from_slice(d))?;
    if !String::from_utf8_lossy(&out).contains("Success") { let _ = a.exec(&format!("pm install-abandon {sid}")); return Err("APK yazılamadı".into()); }
    let r = a.exec(&format!("pm install-commit {sid}"))?;
    if r.contains("Success") { Ok("KernelSU yöneticisi kuruldu".into()) } else { Err(format!("kurulum başarısız: {r}")) }
}
