//! Android OTA payload.bin extractor (full OTAs: REPLACE / BZ / XZ / ZSTD / ZERO).
//! Works on .zip (stored payload.bin) or raw payload.bin. Operations are independent, so they are
//! decompressed on all CPU cores (pread/pwrite) using the native C decoders.
use prost::Message;
use serde::Serialize;
use std::{fs::File, io::{Read, Seek, SeekFrom}, os::unix::fs::FileExt, sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, Mutex}, time::Duration};
use tauri::{AppHandle, Emitter};

pub static CANCEL: AtomicBool = AtomicBool::new(false);
fn e<E: std::fmt::Display>(x: E) -> String { x.to_string() }

#[derive(Clone, PartialEq, Message)]
struct Extent { #[prost(uint64, optional, tag = "1")] start_block: Option<u64>, #[prost(uint64, optional, tag = "2")] num_blocks: Option<u64> }
#[derive(Clone, PartialEq, Message)]
struct Op {
    #[prost(int32, optional, tag = "1")] t: Option<i32>,
    #[prost(uint64, optional, tag = "2")] data_offset: Option<u64>,
    #[prost(uint64, optional, tag = "3")] data_length: Option<u64>,
    #[prost(message, repeated, tag = "6")] dst: Vec<Extent>,
}
#[derive(Clone, PartialEq, Message)]
struct PInfo { #[prost(uint64, optional, tag = "1")] size: Option<u64> }
#[derive(Clone, PartialEq, Message)]
struct Part {
    #[prost(string, optional, tag = "1")] name: Option<String>,
    #[prost(message, optional, tag = "7")] new_info: Option<PInfo>,
    #[prost(message, repeated, tag = "8")] ops: Vec<Op>,
}
#[derive(Clone, PartialEq, Message)]
struct Manifest { #[prost(uint32, optional, tag = "3")] block_size: Option<u32>, #[prost(message, repeated, tag = "13")] parts: Vec<Part> }

struct Src { file: File, data_base: u64, manifest: Manifest }

fn open(path: &str) -> Result<Src, String> {
    let mut file = File::open(path).map_err(|x| format!("{path}: {x}"))?;
    let mut m = [0u8; 4];
    file.read_exact(&mut m).map_err(e)?;
    let mut base = 0u64;
    if &m == b"PK\x03\x04" {
        let mut z = zip::ZipArchive::new(File::open(path).map_err(e)?).map_err(e)?;
        let f = z.by_name("payload.bin").map_err(|_| "zip içinde payload.bin yok".to_string())?;
        if f.compression() != zip::CompressionMethod::Stored { return Err("payload.bin sıkıştırılmış, desteklenmiyor".into()); }
        base = f.data_start();
    }
    file.seek(SeekFrom::Start(base)).map_err(e)?;
    let mut h = [0u8; 24];
    file.read_exact(&mut h).map_err(e)?;
    if &h[..4] != b"CrAU" { return Err("geçerli bir payload.bin değil".into()); }
    if u64::from_be_bytes(h[4..12].try_into().unwrap()) != 2 { return Err("desteklenmeyen payload sürümü".into()); }
    let man = u64::from_be_bytes(h[12..20].try_into().unwrap());
    let sig = u32::from_be_bytes(h[20..24].try_into().unwrap()) as u64;
    let mut mb = vec![0u8; man as usize];
    file.read_exact(&mut mb).map_err(e)?;
    let manifest = Manifest::decode(&mb[..]).map_err(e)?;
    Ok(Src { file, data_base: base + 24 + man + sig, manifest })
}

#[derive(Serialize)]
pub struct PartInfo { pub name: String, pub size: u64 }

pub fn list(path: &str) -> Result<Vec<PartInfo>, String> {
    let s = open(path)?;
    Ok(s.manifest.parts.iter().filter_map(|p| {
        let n = p.name.clone()?;
        if n == "super" { return None; }
        Some(PartInfo { name: n, size: p.new_info.as_ref().and_then(|i| i.size).unwrap_or(0) })
    }).collect())
}

fn decode(t: i32, raw: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut v = Vec::with_capacity(raw.len() * 3);
    match t {
        0 => return Ok(raw),
        1 => { bzip2::read::BzDecoder::new(&raw[..]).read_to_end(&mut v).map_err(e)?; }
        8 => { xz2::read::XzDecoder::new(&raw[..]).read_to_end(&mut v).map_err(e)?; }
        14 => { return zstd::stream::decode_all(&raw[..]).map_err(e); }
        _ => return Err(format!("desteklenmeyen işlem türü {t} (artımlı OTA desteklenmez, tam OTA kullan)")),
    }
    Ok(v)
}

fn do_op(src: &File, base: u64, out: &File, op: &Op, bs: u64) -> Result<(), String> {
    let mut raw = vec![0u8; op.data_length.unwrap_or(0) as usize];
    src.read_exact_at(&mut raw, base + op.data_offset.unwrap_or(0)).map_err(e)?;
    let data = decode(op.t.unwrap_or(0), raw)?;
    let mut pos = 0usize;
    for x in &op.dst {
        let end = (pos + (x.num_blocks.unwrap_or(0) * bs) as usize).min(data.len());
        out.write_all_at(&data[pos..end], x.start_block.unwrap_or(0) * bs).map_err(e)?;
        pos = end;
    }
    Ok(())
}

pub fn extract(app: &AppHandle, path: &str, out: &str, names: &[String]) -> Result<String, String> {
    CANCEL.store(false, Ordering::SeqCst);
    let s = open(path)?;
    let bs = s.manifest.block_size.unwrap_or(4096) as u64;
    std::fs::create_dir_all(out).map_err(e)?;
    let parts: Vec<&Part> = s.manifest.parts.iter()
        .filter(|p| p.name.as_ref().map_or(false, |n| names.contains(n) && n != "super")).collect();
    let mut files: Vec<File> = vec![];
    for p in &parts {
        let name = p.name.clone().unwrap();
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') { return Err(format!("geçersiz bölüm adı: {name}")); }
        let o = File::create(format!("{out}/{name}.img")).map_err(e)?;
        o.set_len(p.new_info.as_ref().and_then(|i| i.size).unwrap_or(0)).map_err(e)?;
        files.push(o);
    }
    // ZERO/DISCARD operations need no work: the preallocated file is already zero.
    let tasks: Vec<(usize, &Op)> = parts.iter().enumerate()
        .flat_map(|(i, p)| p.ops.iter().map(move |o| (i, o))).filter(|(_, o)| !matches!(o.t.unwrap_or(0), 6 | 7)).collect();
    let total = tasks.len();
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let err: Mutex<Option<String>> = Mutex::new(None);
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
    std::thread::scope(|sc| {
        for _ in 0..workers {
            sc.spawn(|| loop {
                if CANCEL.load(Ordering::Relaxed) || err.lock().unwrap().is_some() { break; }
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= total { break; }
                let (pi, op) = tasks[i];
                if let Err(x) = do_op(&s.file, s.data_base, &files[pi], op, bs) {
                    *err.lock().unwrap() = Some(format!("{}: {x}", parts[pi].name.clone().unwrap_or_default()));
                    break;
                }
                done.fetch_add(1, Ordering::SeqCst);
            });
        }
        while total > 0 && done.load(Ordering::SeqCst) < total && err.lock().unwrap().is_none() && !CANCEL.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(150));
            let i = next.load(Ordering::SeqCst).min(total).saturating_sub(1);
            let n = parts[tasks[i].0].name.clone().unwrap_or_default();
            let _ = app.emit("payload-progress", (n.as_str(), done.load(Ordering::SeqCst), total));
        }
    });
    if CANCEL.load(Ordering::SeqCst) { return Err("iptal edildi".into()); }
    if let Some(x) = err.into_inner().unwrap() { return Err(x); }
    let _ = app.emit("payload-progress", ("", total, total));
    Ok(out.to_string())
}
