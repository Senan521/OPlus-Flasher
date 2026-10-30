//! ADB host protocol over USB (CNXN/AUTH/OPEN/WRTE/OKAY/CLSE) using a USB fd from UsbManager.
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use futures_lite::future::block_on;
use nusb::{transfer::RequestBuffer, Interface};
use rsa::{pkcs8::{DecodePrivateKey, EncodePrivateKey}, traits::PublicKeyParts, BigUint, Pkcs1v15Sign, RsaPrivateKey};
use sha1::Sha1;
use std::{cell::RefCell, collections::VecDeque, io::Read, os::fd::{FromRawFd, OwnedFd}, path::Path, time::Duration};

const CNXN: u32 = 0x4e584e43; const AUTH: u32 = 0x48545541; const OPEN: u32 = 0x4e45504f;
const OKAY: u32 = 0x59414b4f; const CLSE: u32 = 0x45534c43; const WRTE: u32 = 0x45545257;
const MAXDATA: u32 = 1024 * 1024;
fn e<E: std::fmt::Display>(x: E) -> String { x.to_string() }

pub struct Adb { iface: Interface, ep_in: u8, ep_out: u8, rx: VecDeque<u8>, mps: usize, max: usize, next: u32, hist: RefCell<VecDeque<String>> }
fn cn(c: u32) -> &'static str { match c { CNXN => "CNXN", AUTH => "AUTH", OPEN => "OPEN", OKAY => "OKAY", CLSE => "CLSE", WRTE => "WRTE", _ => "????" } }
struct Pkt { cmd: u32, a0: u32, a1: u32, data: Vec<u8> }

fn load_key(p: &Path) -> Result<RsaPrivateKey, String> {
    if let Ok(b) = std::fs::read(p) { if let Ok(k) = RsaPrivateKey::from_pkcs8_der(&b) { return Ok(k); } }
    let k = RsaPrivateKey::new(&mut rand_core::OsRng, 2048).map_err(e)?;
    std::fs::write(p, k.to_pkcs8_der().map_err(e)?.as_bytes()).map_err(e)?;
    Ok(k)
}

/// Android's "RSAPublicKey" struct (2048-bit): len, n0inv, n[64], rr[64], exponent.
fn android_pubkey(k: &RsaPrivateKey) -> Vec<u8> {
    let words = |x: &BigUint| {
        let bytes = x.to_bytes_le();
        let mut d = vec![0u32; 64];

        for (i, chunk) in bytes.chunks(4).enumerate().take(64) {
            let mut word = [0u8; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            d[i] = u32::from_le_bytes(word);
        }

        d
    };
    let n = k.n();
    let nd = words(n);
    let n0 = nd[0];
    let mut inv = 1u32;
    for _ in 0..5 { inv = inv.wrapping_mul(2u32.wrapping_sub(n0.wrapping_mul(inv))); }
    let rr = (BigUint::from(1u32) << 4096usize) % n;
    let mut o = vec![];
    o.extend(64u32.to_le_bytes());
    o.extend(inv.wrapping_neg().to_le_bytes());
    for w in nd { o.extend(w.to_le_bytes()); }
    for w in words(&rr) { o.extend(w.to_le_bytes()); }
    o.extend(65537u32.to_le_bytes());
    o
}

impl Adb {
    pub fn open(fd: i32, iface: u8, ep_in: u8, ep_out: u8, mps: u32, key_path: &Path, on_auth: &dyn Fn()) -> Result<Self, String> {
        let dup = unsafe { libc::dup(fd) };
        if dup < 0 { return Err("dup() failed".into()); }
        let dev = nusb::Device::from_fd(unsafe { OwnedFd::from_raw_fd(dup) }).map_err(e)?;
        let iface = dev.claim_interface(iface).map_err(e)?;
        let mut a = Self { iface, ep_in, ep_out, rx: VecDeque::new(), mps: (mps as usize).max(64), max: 4096, next: 1, hist: RefCell::new(VecDeque::new()) };
        let key = load_key(key_path)?;
        a.send(CNXN, 0x01000001, MAXDATA, b"host::\0")?;
        let (mut signed, mut pubs) = (false, 0);
        loop {
            let p = a.read()?;
            match p.cmd {
                CNXN => { a.max = (p.a1 as usize).clamp(4096, MAXDATA as usize); return Ok(a); }
                AUTH if p.a0 == 1 => {
                    if !signed {
                        let sig = key.sign(Pkcs1v15Sign::new::<Sha1>(), &p.data).map_err(e)?;
                        a.send(AUTH, 2, 0, &sig)?; signed = true;
                    } else {
                        if pubs >= 2 { return Err("USB hata ayıklama izni verilmedi".into()); }
                        let mut pk = B64.encode(android_pubkey(&key)).into_bytes();
                        pk.extend(b" oplus-flasher@android\0");
                        a.send(AUTH, 3, 0, &pk)?; pubs += 1; on_auth();
                    }
                }
                _ => {}
            }
        }
    }
    fn note(&self, m: String) { let mut h = self.hist.borrow_mut(); if h.len() >= 16 { h.pop_front(); } h.push_back(m); }
    fn dump(&self) -> String { self.hist.borrow().iter().cloned().collect::<Vec<_>>().join(" | ") }
    fn out(&self, d: Vec<u8>) -> Result<(), String> { block_on(self.iface.bulk_out(self.ep_out, d)).into_result().map(|_| ()).map_err(e) }
    fn send(&self, cmd: u32, a0: u32, a1: u32, data: &[u8]) -> Result<(), String> {
        self.note(format!("→{} {a0} {a1} {}B", cn(cmd), data.len()));
        let mut h = Vec::with_capacity(24);
        for v in [cmd, a0, a1, data.len() as u32, data.iter().map(|b| *b as u32).fold(0u32, |s, b| s.wrapping_add(b)), cmd ^ 0xffff_ffff] { h.extend(v.to_le_bytes()); }
        self.out(h)?;
        if !data.is_empty() { self.out(data.to_vec())?; }
        Ok(())
    }
    fn fill(&mut self, n: usize) -> Result<(), String> {
        // The transfer length must be a multiple of the endpoint packet size, and it must not be larger than what the
        // device will still send in whole packets, otherwise the transfer waits forever for more data.
        let mps = self.mps;
        while self.rx.len() < n {
            let rem = n - self.rx.len();
            let len = (((rem + mps - 1) / mps) * mps).min(262_144 / mps * mps).max(mps);
            let v = block_on(self.iface.bulk_in(self.ep_in, RequestBuffer::new(len))).into_result().map_err(e)?;
            self.rx.extend(v);
        }
        Ok(())
    }
    fn read(&mut self) -> Result<Pkt, String> {
        self.fill(24)?;
        let h: Vec<u8> = self.rx.drain(..24).collect();
        let w = |i: usize| u32::from_le_bytes(h[i * 4..i * 4 + 4].try_into().unwrap());
        let len = w(3) as usize;
        self.fill(len)?;
        let p = Pkt { cmd: w(0), a0: w(1), a1: w(2), data: self.rx.drain(..len).collect() };
        self.note(format!("←{} {} {} {}B", cn(p.cmd), p.a0, p.a1, len));
        Ok(p)
    }
    /// Packets that belong to another (stale) stream are ignored; stray WRTEs are acknowledged so the device never stalls.
    fn foreign(&self, p: &Pkt) -> Result<(), String> { if p.cmd == WRTE { self.send(OKAY, p.a1, p.a0, &[])?; } Ok(()) }

    /// Runs a service (e.g. "exec:ls /sdcard"), optionally feeding stdin from `input`, streaming output to `sink`.
    /// A rejected OPEN is retried (adbd can refuse while it is busy).
    pub fn run(&mut self, svc: &str, mut input: Option<&mut dyn Read>, sink: &mut dyn FnMut(&[u8])) -> Result<(), String> {
        let mut tries = 0;
        let (id, remote) = loop {
            let id = self.next; self.next += 1;
            self.send(OPEN, id, 0, format!("{svc}\0").as_bytes())?;
            let ans = loop {
                let p = self.read()?;
                if p.a1 != id { self.foreign(&p)?; continue; }
                match p.cmd { OKAY => break Some(p.a0), CLSE => break None, _ => {} }
            };
            match ans {
                Some(r) => break (id, r),
                None => {
                    tries += 1;
                    if tries >= 3 { return Err(format!("servis reddedildi: {svc} [{}]", self.dump())); }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        };
        let (mut waiting, mut buf) = (false, vec![0u8; self.max]);
        loop {
            if crate::backup::CANCEL.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = self.send(CLSE, id, remote, &[]);
                return Err("iptal edildi".into());
            }
            let mut eof = false;
            if !waiting {
                if let Some(r) = input.as_mut() {
                    let n = r.read(&mut buf).map_err(e)?;
                    if n > 0 { self.send(WRTE, id, remote, &buf[..n])?; waiting = true; } else { eof = true; }
                }
            }
            if eof { input = None; }
            let p = self.read()?;
            if p.a1 != id { self.foreign(&p)?; continue; }
            match p.cmd {
                OKAY => waiting = false,
                WRTE => { sink(&p.data); self.send(OKAY, id, remote, &[])?; }
                CLSE => { let _ = self.send(CLSE, id, remote, &[]); return Ok(()); }
                _ => {}
            }
        }
    }
    /// Text command. Falls back to the pty-based `shell:` service when `exec:` is refused.
    pub fn exec(&mut self, cmd: &str) -> Result<String, String> {
        let mut o = vec![];
        let r = self.run(&format!("exec:{cmd}"), None, &mut |d| o.extend_from_slice(d));
        match r {
            Ok(()) => Ok(String::from_utf8_lossy(&o).to_string()),
            Err(x) if x.starts_with("servis reddedildi") => {
                let mut o2 = vec![];
                let r2 = self.run(&format!("shell:{cmd}"), None, &mut |d| o2.extend_from_slice(d));
                r2.map_err(|y| format!("{x}; shell: {y}"))?;
                Ok(String::from_utf8_lossy(&o2).replace("\r\n", "\n"))
            }
            Err(x) => Err(x),
        }
    }
}
