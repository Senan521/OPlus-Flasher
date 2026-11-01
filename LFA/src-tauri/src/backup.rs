//! App backup/restore over the ADB-over-USB layer (target device needs root). Archives are written on THIS phone.
use crate::adb::Adb;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs::{self, File}, io::{Read, Write}, path::{Component, Path, PathBuf}, sync::atomic::{AtomicBool, Ordering}};
use tauri::{AppHandle, Emitter};

pub static CANCEL: AtomicBool = AtomicBool::new(false);
fn e<E: std::fmt::Display>(x: E) -> String { x.to_string() }
const KINDS: [(&str, &str); 3] = [("data", "/data/user/0"), ("de", "/data/user_de/0"), ("ext", "/data/media/0/Android/data")];

#[derive(Serialize, Deserialize)]
struct Manifest { pkg: String, time: u64, apks: Vec<String>, kinds: Vec<String> }
/// Uygulama klasöründe durmayan, sistemin tuttuğu ayarlar: çalışma zamanı izinleri, AppOps ve SSAID (perms.json).
#[derive(Serialize, Deserialize, Default)]
struct Perms { #[serde(default)] runtime: Vec<String>, #[serde(default)] appops: Vec<(String, String)>, #[serde(default)] ssaid: Option<String> }
const SSAID_FILE: &str = "/data/system/users/0/settings_ssaid.xml";
fn valid_perm(p: &str) -> bool { !p.is_empty() && p.len() < 200 && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') }
fn valid_op(p: &str) -> bool { !p.is_empty() && p.len() < 80 && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') }

/// `dumpsys package` çıktısındaki ilk "runtime permissions:" bloğundan (kullanıcı 0) verilmiş izinleri alır.
fn parse_runtime(dump: &str) -> Vec<String> {
    let mut out = vec![];
    let mut base: Option<usize> = None;
    for l in dump.lines() {
        let t = l.trim();
        let ind = l.len() - l.trim_start().len();
        match base {
            None => { if t == "runtime permissions:" { base = Some(ind); } }
            Some(b) => {
                if t.is_empty() { continue; }
                if ind <= b { break; }
                if let Some((name, rest)) = t.split_once(':') {
                    if rest.contains("granted=true") && valid_perm(name) { out.push(name.to_string()); }
                }
            }
        }
    }
    out
}
/// `appops get <pkg>` çıktısı: "OP: mod; time=..." satırları ("default" ve uid modları hariç).
fn parse_appops(o: &str) -> Vec<(String, String)> {
    o.lines().filter_map(|l| {
        let t = l.trim();
        if t.starts_with("Uid mode") { return None; }
        let (op, rest) = t.split_once(':')?;
        let mode = rest.trim().split(|c: char| c == ';' || c.is_whitespace()).next()?;
        if valid_op(op) && ["allow", "ignore", "deny", "foreground"].contains(&mode) { Some((op.to_string(), mode.to_string())) } else { None }
    }).collect()
}
fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let i = tag.find(&key)? + key.len();
    Some(tag[i..].split('"').next()?.to_string())
}
fn ssaid_find(xml: &str, pkg: &str) -> Option<String> {
    xml.split('<').filter(|t| t.starts_with("setting ")).find(|t| attr(t, "package").as_deref() == Some(pkg))
        .and_then(|t| attr(t, "value")).filter(|v| !v.is_empty() && v.len() <= 64 && v.chars().all(|c| c.is_ascii_hexdigit()))
}
fn ssaid_read(a: &mut Adb, cx: &Cx) -> Result<String, String> {
    a.exec(&cx.c(&format!("abx2xml {SSAID_FILE} - 2>/dev/null || cat {SSAID_FILE}")))
}

#[derive(Serialize)]
pub struct Item { pub pkg: String, pub time: u64, pub mb: u64 }

pub fn valid_pkg(p: &str) -> bool {
    let segs: Vec<&str> = p.split('.').collect();
    segs.len() >= 2 && p.len() <= 200 && segs.iter().all(|s| !s.is_empty() && !s.starts_with(|c: char| c.is_ascii_digit()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}
fn valid_apk_name(n: &str) -> bool { n.ends_with(".apk") && !n.contains("..") && n.chars().all(|c| c.is_ascii_alphanumeric() || "_.=-".contains(c)) }

struct Cx { su: bool }
impl Cx {
    fn new(a: &mut Adb) -> Result<Self, String> {
        let id = a.exec("id").unwrap_or_default();
        if id.contains("uid=0") { return Ok(Cx { su: false }); }
        let mut notes = vec![format!("id → {}", id.trim())];
        match a.exec("su -c 'id'") {
            Ok(o) if o.contains("uid=0") => return Ok(Cx { su: true }),
            Ok(o) => notes.push(format!("su → {}", o.trim())),
            Err(x) => notes.push(format!("su → {x}")),
        }
        Err(format!("Root doğrulanamadı ({}). Magisk/KernelSU’da Shell’e root izni ver, hedef ekranı açık tut.", notes.join("; ")))
    }
    fn c(&self, cmd: &str) -> String { if self.su { format!("su -c '{cmd}'") } else { cmd.to_string() } }
}

/// Only entries inside <pkg>/ are allowed, no `..`, and nothing may be written through a symlink.
fn verify_tar(p: &Path, pkg: &str) -> Result<(), String> {
    let mut ar = tar::Archive::new(File::open(p).map_err(e)?);
    let mut links: HashSet<PathBuf> = HashSet::new();
    for en in ar.entries().map_err(e)? {
        let en = en.map_err(e)?;
        let path = en.path().map_err(e)?.to_path_buf();
        let mut comps = path.components();
        if !matches!(comps.next(), Some(Component::Normal(c)) if c == pkg) || comps.any(|c| !matches!(c, Component::Normal(_))) {
            return Err(format!("güvensiz arşiv girdisi: {}", path.display()));
        }
        if path.ancestors().skip(1).any(|a| links.contains(a)) { return Err(format!("symlink üzerinden yazım: {}", path.display())); }
        let t = en.header().entry_type();
        if t.is_symlink() { links.insert(path.clone()); }
        if t.is_hard_link() {
            let l = en.link_name().map_err(e)?.map(|l| l.to_path_buf()).unwrap_or_default();
            if l.is_absolute() || l.components().any(|c| matches!(c, Component::ParentDir)) { return Err("güvensiz hardlink".into()); }
        }
    }
    Ok(())
}

pub fn check_root(a: &mut Adb) -> Result<String, String> { Ok(if Cx::new(a)?.su { "su" } else { "adb root" }.to_string()) }

pub fn packages(a: &mut Adb) -> Result<Vec<String>, String> {
    let mut v: Vec<String> = a.exec("pm list packages -3")?.lines().filter_map(|l| l.trim().strip_prefix("package:")).filter(|p| valid_pkg(p)).map(String::from).collect();
    if v.is_empty() {
        let alt = a.exec("pm list packages -3 2>&1")?;
        return Err(format!("Uygulama listesi boş. Çıktı: {}", alt.trim().chars().take(300).collect::<String>()));
    }
    v.sort();
    Ok(v)
}

struct Prog<'a> { app: &'a AppHandle, pkg: String, idx: usize, n: usize, total: u64, done: u64, last: std::time::Instant }
impl Prog<'_> {
    fn add(&mut self, b: usize) {
        self.done += b as u64;
        if self.last.elapsed().as_millis() >= 150 {
            self.last = std::time::Instant::now();
            let f = if self.total > 0 { (self.done as f64 / self.total as f64).min(0.99) } else { 0.0 };
            let _ = self.app.emit("bk-progress", (self.pkg.as_str(), self.idx as f64 + f, self.n as f64));
        }
    }
}

fn to_file(a: &mut Adb, svc: &str, path: &Path, pr: &mut Prog) -> Result<(), String> {
    let mut f = File::create(path).map_err(e)?;
    let mut err = None;
    a.run(svc, None, &mut |d| { if let Err(x) = f.write_all(d) { err = Some(x.to_string()); } pr.add(d.len()); })?;
    err.map_or(Ok(()), Err)
}

fn backup_pkg(app: &AppHandle, a: &mut Adb, cx: &Cx, pkg: &str, idx: usize, n: usize, out: &Path) -> Result<(), String> {
    let dir = out.join(pkg);
    fs::create_dir_all(&dir).map_err(e)?;
    let mut paths = vec![];
    for l in a.exec(&format!("pm path {pkg}"))?.lines() {
        let p = l.trim().strip_prefix("package:").ok_or("pm path hatası")?;
        if !p.starts_with("/data/app/") || p.contains("..") || !p.ends_with(".apk") || !p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-=~".contains(c)) {
            return Err(format!("beklenmeyen APK yolu: {p}"));
        }
        paths.push(p.to_string());
    }
    if paths.is_empty() { return Err("APK yolu bulunamadı".into()); }
    // size estimate for the progress bar
    let mut total = 0u64;
    for p in &paths { total += a.exec(&format!("stat -c %s {p}"))?.trim().parse::<u64>().unwrap_or(0); }
    let mut present = vec![];
    for (k, base) in KINDS {
        let path = format!("{base}/{pkg}");
        if !a.exec(&cx.c(&format!("test -d {path} && echo y")))?.trim().starts_with('y') { continue; }
        let kb = a.exec(&cx.c(&format!("timeout 6 du -sk {path} 2>/dev/null"))).ok()
            .and_then(|o| o.split_whitespace().next().and_then(|x| x.parse::<u64>().ok())).unwrap_or(0);
        total += kb * 1024;
        present.push((k, base));
    }
    let mut pr = Prog { app, pkg: pkg.to_string(), idx, n, total, done: 0, last: std::time::Instant::now() };
    let mut apks = vec![];
    for p in &paths {
        let name = p.rsplit('/').next().unwrap().to_string();
        to_file(a, &format!("exec:cat {p}"), &dir.join(&name), &mut pr)?;
        apks.push(name);
    }
    let mut kinds = vec![];
    for (k, base) in present {
        let f = dir.join(format!("{k}.tar"));
        to_file(a, &format!("exec:{} 2>/dev/null", cx.c(&format!("tar -c -C {base} --exclude=\"*/cache\" --exclude=\"*/code_cache\" --exclude=\"*/Cache\" --exclude=\"*/GPUCache\" --exclude=\"*/shader_cache\" {pkg}"))), &f, &mut pr)?;
        if fs::metadata(&f).map(|m| m.len()).unwrap_or(0) < 512 { return Err(format!("{k}.tar boş (tar komutu çalışmadı)")); }
        verify_tar(&f, pkg)?;
        kinds.push(k.to_string());
    }
    bk_log(app, format!("{pkg}: izinler, AppOps ve SSAID okunuyor"));
    let mut perms = Perms::default();
    if let Ok(d) = a.exec(&format!("dumpsys package {pkg}")) { perms.runtime = parse_runtime(&d); }
    if let Ok(o) = a.exec(&format!("appops get {pkg}")) { perms.appops = parse_appops(&o); }
    if let Ok(x) = ssaid_read(a, cx) { perms.ssaid = ssaid_find(&x, pkg); }
    bk_log(app, format!("{pkg}: {} izin, {} AppOps{}", perms.runtime.len(), perms.appops.len(), if perms.ssaid.is_some() { ", SSAID" } else { "" }));
    fs::write(dir.join("perms.json"), serde_json::to_vec_pretty(&perms).map_err(e)?).map_err(e)?;
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&Manifest { pkg: pkg.into(), time, apks, kinds }).map_err(e)?).map_err(e)
}

pub fn backup(app: &AppHandle, a: &mut Adb, pkgs: &[String], out: &str) -> Result<(), String> {
    CANCEL.store(false, Ordering::SeqCst);
    let r = backup_all(app, a, pkgs, out);
    CANCEL.store(false, Ordering::SeqCst); // a stale flag would abort every later ADB command
    r
}
fn backup_all(app: &AppHandle, a: &mut Adb, pkgs: &[String], out: &str) -> Result<(), String> {
    let cx = Cx::new(a)?;
    for (i, p) in pkgs.iter().enumerate() {
        if !valid_pkg(p) { return Err(format!("geçersiz paket adı: {p}")); }
        if CANCEL.load(Ordering::SeqCst) { return Err("iptal edildi".into()); }
        let _ = app.emit("bk-progress", (p.as_str(), i as f64, pkgs.len() as f64));
        if let Err(x) = backup_pkg(app, a, &cx, p, i, pkgs.len(), Path::new(out)) {
            let _ = fs::remove_dir_all(Path::new(out).join(p)); // no half-written backups
            return Err(format!("{p}: {x}"));
        }
    }
    let _ = app.emit("bk-progress", ("", pkgs.len() as f64, pkgs.len() as f64));
    Ok(())
}

pub fn list(out: &str) -> Result<Vec<Item>, String> {
    let mut v = vec![];
    for en in fs::read_dir(out).map_err(|x| format!("{out}: {x}"))?.flatten() {
        let Ok(b) = fs::read(en.path().join("manifest.json")) else { continue };
        let Ok(m) = serde_json::from_slice::<Manifest>(&b) else { continue };
        let size: u64 = fs::read_dir(en.path()).map(|r| r.flatten().filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0);
        v.push(Item { pkg: m.pkg, time: m.time, mb: size / 1_000_000 });
    }
    v.sort_by(|a, b| a.pkg.cmp(&b.pkg));
    Ok(v)
}

pub(crate) fn feed(a: &mut Adb, svc: &str, input: &mut dyn Read) -> Result<String, String> {
    let mut o = vec![];
    a.run(svc, Some(input), &mut |d| o.extend_from_slice(d))?;
    Ok(String::from_utf8_lossy(&o).to_string())
}
struct CountRead<'a, 'b> { f: File, pr: &'a mut Prog<'b> }
impl Read for CountRead<'_, '_> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> { let n = self.f.read(b)?; self.pr.add(n); Ok(n) }
}
fn bk_log(app: &AppHandle, m: String) { let _ = app.emit("bk-log", m); }

fn restore_pkg(app: &AppHandle, a: &mut Adb, cx: &Cx, pkg: &str, dir: &Path, idx: usize, n: usize, ssaid: bool) -> Result<(), String> {
    let m: Manifest = serde_json::from_slice(&fs::read(dir.join("manifest.json")).map_err(e)?).map_err(e)?;
    if m.pkg != pkg { return Err("manifest paketle uyuşmuyor".into()); }
    let sizes: Vec<u64> = m.apks.iter().map(|x| if valid_apk_name(x) { fs::metadata(dir.join(x)).map(|y| y.len()).map_err(e) } else { Err(format!("geçersiz APK adı: {x}")) }).collect::<Result<_, _>>()?;
    for k in &m.kinds {
        if !KINDS.iter().any(|(nm, _)| *nm == k.as_str()) { return Err("geçersiz arşiv türü".into()); }
        verify_tar(&dir.join(format!("{k}.tar")), pkg)?;
    }
    let installed = a.exec(&format!("pm path {pkg}")).map(|o| o.contains("package:")).unwrap_or(false);
    let tars: Vec<(String, u64)> = m.kinds.iter().map(|k| (k.clone(), fs::metadata(dir.join(format!("{k}.tar"))).map(|x| x.len()).unwrap_or(0))).collect();
    let apk_total: u64 = sizes.iter().sum();
    let total = (if installed { 0 } else { apk_total }) + tars.iter().map(|x| x.1).sum::<u64>();
    let mut pr = Prog { app, pkg: pkg.to_string(), idx, n, total, done: 0, last: std::time::Instant::now() };
    if installed {
        bk_log(app, format!("{pkg}: uygulama zaten kurulu, APK adımı atlandı"));
    } else {
        bk_log(app, format!("{pkg}: APK kuruluyor"));
        let o = a.exec(&format!("pm install-create -r -t -S {apk_total}"))?;
        let sid: String = o.split('[').nth(1).and_then(|x| x.split(']').next()).unwrap_or("").to_string();
        if sid.is_empty() || !sid.chars().all(|c| c.is_ascii_digit()) { return Err(format!("kurulum oturumu açılamadı: {}", o.trim())); }
        for (i, nme) in m.apks.iter().enumerate() {
            let mut rd = CountRead { f: File::open(dir.join(nme)).map_err(e)?, pr: &mut pr };
            let r = feed(a, &format!("exec:pm install-write -S {} {sid} {i}.apk -", sizes[i]), &mut rd)?;
            if !r.contains("Success") { let _ = a.exec(&format!("pm install-abandon {sid}")); return Err(format!("APK yazılamadı: {}", r.trim())); }
        }
        let r = a.exec(&format!("pm install-commit {sid}"))?;
        if !r.contains("Success") {
            let hint = if r.contains("USER_RESTRICTED") { " (hedef cihazda Geliştirici seçenekleri > “USB ile yüklemeye izin ver” açık olmalı)" } else { "" };
            return Err(format!("kurulum başarısız: {}{hint}", r.trim()));
        }
    }
    let _ = a.exec(&format!("am force-stop {pkg}"));
    for (k, sz) in &tars {
        let base = KINDS.iter().find(|(nm, _)| *nm == k.as_str()).unwrap().1;
        bk_log(app, format!("{pkg}: veri geri yükleniyor ({k})"));
        let tmp = format!("/data/local/tmp/lf_{k}.tar");
        {
            let mut rd = CountRead { f: File::open(dir.join(format!("{k}.tar"))).map_err(e)?, pr: &mut pr };
            feed(a, &format!("exec:head -c {sz} > {tmp}"), &mut rd)?;
        }
        let got = a.exec(&format!("stat -c %s {tmp}"))?.trim().parse::<u64>().unwrap_or(0);
        if got != *sz { let _ = a.exec(&format!("rm -f {tmp}")); return Err(format!("{k}.tar cihaza eksik gönderildi ({got}/{sz})")); }
        let r = a.exec(&cx.c(&format!("tar -x -C {base} -f {tmp} 2>&1; echo EXIT:$?")))?;
        let _ = a.exec(&format!("rm -f {tmp}"));
        if !r.trim_end().ends_with("EXIT:0") { return Err(format!("tar -x başarısız: {}", r.trim())); }
        if k != "ext" {
            let uid = a.exec(&cx.c(&format!("stat -c %u {base}/{pkg}")))?.trim().to_string();
            if uid.is_empty() || !uid.chars().all(|c| c.is_ascii_digit()) { return Err("uid okunamadı".into()); }
            a.exec(&cx.c(&format!("chown -R {uid}:{uid} {base}/{pkg}")))?;
        }
        a.exec(&cx.c(&format!("restorecon -RF {base}/{pkg}")))?;
    }
    if let Some(p) = fs::read(dir.join("perms.json")).ok().and_then(|b| serde_json::from_slice::<Perms>(&b).ok()) { apply_perms(app, a, cx, pkg, &p, ssaid); }
    Ok(())
}

/// Hatalar ölümcül değildir: günlüğe uyarı olarak yazılır (bazı izinler/işlemler cihazda değiştirilemez).
fn apply_perms(app: &AppHandle, a: &mut Adb, cx: &Cx, pkg: &str, p: &Perms, ssaid: bool) {
    let rt: Vec<&String> = p.runtime.iter().filter(|x| valid_perm(x)).collect();
    if !rt.is_empty() {
        bk_log(app, format!("{pkg}: {} çalışma zamanı izni veriliyor", rt.len()));
        for ch in rt.chunks(12) {
            let _ = a.exec(&ch.iter().map(|x| format!("pm grant {pkg} {x}")).collect::<Vec<_>>().join("; "));
        }
    }
    let ops: Vec<&(String, String)> = p.appops.iter().filter(|(o, m)| valid_op(o) && ["allow", "ignore", "deny", "foreground"].contains(&m.as_str())).collect();
    if !ops.is_empty() {
        bk_log(app, format!("{pkg}: {} AppOps ayarlanıyor", ops.len()));
        for ch in ops.chunks(12) {
            let _ = a.exec(&ch.iter().map(|(o, m)| format!("appops set {pkg} {o} {m}")).collect::<Vec<_>>().join("; "));
        }
    }
    if ssaid {
        if let Some(v) = &p.ssaid {
            match restore_ssaid(a, cx, pkg, v) {
                Ok(()) => bk_log(app, format!("{pkg}: SSAID yazıldı, geçerli olması için cihazı yeniden başlat")),
                Err(x) => bk_log(app, format!("⚠ {pkg}: SSAID geri yüklenemedi: {x}")),
            }
        } else { bk_log(app, format!("{pkg}: yedekte SSAID yok")); }
    }
}

/// settings_ssaid.xml içindeki kaydı değiştirir (yoksa ekler). Dosya ABX ise abx2xml/xml2abx ile dönüştürülür; yerinde (`cat >`)
/// yazıldığı için sahiplik ve SELinux etiketi korunur. Sistem hafızadaki değeri yeniden başlatmaya kadar kullanır.
fn restore_ssaid(a: &mut Adb, cx: &Cx, pkg: &str, val: &str) -> Result<(), String> {
    if !val.chars().all(|c| c.is_ascii_hexdigit()) { return Err("geçersiz SSAID".into()); }
    let abx = a.exec(&cx.c(&format!("abx2xml {SSAID_FILE} - >/dev/null 2>&1 && echo abx")))?.contains("abx");
    let xml = ssaid_read(a, cx)?;
    if !xml.contains("<settings") { return Err("settings_ssaid.xml okunamadı".into()); }
    let mut tags: Vec<String> = xml.split('<').map(|s| s.to_string()).collect();
    let mut done = false;
    for t in tags.iter_mut() {
        if t.starts_with("setting ") && attr(t, "package").as_deref() == Some(pkg) {
            let old = attr(t, "value").ok_or("value yok")?;
            *t = t.replacen(&format!(" value=\"{old}\""), &format!(" value=\"{val}\""), 1);
            done = true;
        }
    }
    let mut out = tags.join("<");
    if !done {
        let uid = a.exec(&cx.c(&format!("stat -c %u /data/user/0/{pkg}")))?.trim().to_string();
        if uid.is_empty() || !uid.chars().all(|c| c.is_ascii_digit()) { return Err("uid okunamadı".into()); }
        let id = xml.split('<').filter(|t| t.starts_with("setting ")).filter_map(|t| attr(t, "id")?.parse::<u64>().ok()).max().unwrap_or(0) + 1;
        let line = format!("<setting id=\"{id}\" name=\"{uid}\" value=\"{val}\" package=\"{pkg}\" defaultValue=\"{val}\" defaultSysSet=\"true\" tag=\"null\" />\n");
        out = out.replacen("</settings>", &format!("{line}</settings>"), 1);
    }
    let tmp = "/data/local/tmp/lf_ssaid.xml";
    let bin = "/data/local/tmp/lf_ssaid.out";
    let bak = "/data/local/tmp/settings_ssaid.lf.bak";
    let mut rd = std::io::Cursor::new(out.into_bytes());
    let len = rd.get_ref().len();
    feed(a, &format!("exec:head -c {len} > {tmp}"), &mut rd)?;
    a.exec(&cx.c(&format!("cp {SSAID_FILE} {bak}")))?;
    let conv = if abx { format!("xml2abx {tmp} {bin}") } else { format!("cp {tmp} {bin}") };
    let r = a.exec(&cx.c(&format!("{conv} 2>&1; test -s {bin} && echo OK")))?;
    if !r.trim_end().ends_with("OK") { let _ = a.exec(&format!("rm -f {tmp} {bin}")); return Err(format!("dönüştürme başarısız: {}", r.trim())); }
    let r = a.exec(&cx.c(&format!("cat {bin} > {SSAID_FILE} && echo OK")))?;
    let _ = a.exec(&format!("rm -f {tmp} {bin}"));
    if !r.trim_end().ends_with("OK") { return Err(format!("yazılamadı (yedek: {bak}): {}", r.trim())); }
    Ok(())
}

pub fn restore(app: &AppHandle, a: &mut Adb, pkgs: &[String], out: &str, ssaid: bool) -> Result<(), String> {
    CANCEL.store(false, Ordering::SeqCst);
    let r = restore_all(app, a, pkgs, out, ssaid);
    CANCEL.store(false, Ordering::SeqCst);
    r
}
fn restore_all(app: &AppHandle, a: &mut Adb, pkgs: &[String], out: &str, ssaid: bool) -> Result<(), String> {
    let cx = Cx::new(a)?;
    for (i, p) in pkgs.iter().enumerate() {
        if !valid_pkg(p) { return Err(format!("geçersiz paket adı: {p}")); }
        if CANCEL.load(Ordering::SeqCst) { return Err("iptal edildi".into()); }
        let _ = app.emit("bk-progress", (p.as_str(), i as f64, pkgs.len() as f64));
        restore_pkg(app, a, &cx, p, &Path::new(out).join(p), i, pkgs.len(), ssaid).map_err(|x| format!("{p}: {x}"))?;
    }
    let _ = app.emit("bk-progress", ("", pkgs.len() as f64, pkgs.len() as f64));
    Ok(())
}
