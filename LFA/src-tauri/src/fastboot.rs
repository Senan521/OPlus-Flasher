//! Minimal fastboot protocol over a USB fd handed out by Android's UsbManager.
use nusb::{transfer::RequestBuffer, Interface};
use std::{fs::File, future::Future, io::{Read, Seek, SeekFrom}, os::fd::{FromRawFd, OwnedFd}, sync::Arc, task::{Context, Poll, Wake, Waker}, thread, time::{Duration, Instant}};

/// Runs a future to completion on this thread, giving up after `dur`. The future is dropped on timeout (nusb cancels the transfer).
fn wait<F: Future>(f: F, dur: Duration) -> Option<F::Output> {
    struct W(thread::Thread);
    impl Wake for W { fn wake(self: Arc<Self>) { self.0.unpark(); } }
    let mut f = std::pin::pin!(f);
    let waker = Waker::from(Arc::new(W(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let end = Instant::now() + dur;
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return Some(v); }
        let now = Instant::now();
        if now >= end { return None; }
        thread::park_timeout(end - now);
    }
}
/// Many Android kernels / controllers only handle small usbfs URBs reliably (AOSP fastboot also writes 16 KiB at a time).
const CHUNK: usize = 16 * 1024;

pub enum Reply { Okay(String), Data(usize) }
pub struct Fastboot { iface: Interface, ep_in: u8, ep_out: u8 }
fn e<E: std::fmt::Display>(x: E) -> String { x.to_string() }

impl Fastboot {
    pub fn open(fd: i32, iface: u8, ep_in: u8, ep_out: u8) -> Result<Self, String> {
        let dup = unsafe { libc::dup(fd) };
        if dup < 0 { return Err("dup() failed".into()); }
        let dev = nusb::Device::from_fd(unsafe { OwnedFd::from_raw_fd(dup) }).map_err(e)?;
        let iface = dev.claim_interface(iface).map_err(e)?;
        Ok(Self { iface, ep_in, ep_out })
    }
    fn send(&self, d: &[u8]) -> Result<(), String> {
        wait(self.iface.bulk_out(self.ep_out, d.to_vec()), Duration::from_secs(20))
            .ok_or_else(|| format!("USB zaman aşımı (gönderme, {} bayt)", d.len()))?
            .into_result().map(|_| ()).map_err(e)
    }
    /// Sends payload data in small URBs; the error says how far it got.
    fn send_data(&self, d: &[u8], sent: u64, total: u64) -> Result<(), String> {
        let mut off = 0usize;
        for c in d.chunks(CHUNK) {
            self.send(c).map_err(|x| format!("{x} [{}/{} bayt gönderildi]", sent + off as u64, total))?;
            off += c.len();
        }
        Ok(())
    }
    fn recv(&self, secs: u64) -> Result<Vec<u8>, String> {
        wait(self.iface.bulk_in(self.ep_in, RequestBuffer::new(4096)), Duration::from_secs(secs))
            .ok_or_else(|| format!("USB zaman aşımı (cevap, {secs} sn)"))?
            .into_result().map_err(e)
    }
    fn reply(&self, info: &mut Vec<String>, secs: u64) -> Result<Reply, String> {
        loop {
            let r = self.recv(secs)?;
            if r.len() < 4 { return Err("short reply".into()); }
            let (tag, body) = r.split_at(4);
            let body = String::from_utf8_lossy(body).to_string();
            match tag {
                b"INFO" | b"TEXT" => info.push(body),
                b"OKAY" => return Ok(Reply::Okay(body)),
                b"FAIL" => return Err(format!("FAIL: {body}")),
                b"DATA" => return Ok(Reply::Data(usize::from_str_radix(body.trim(), 16).map_err(e)?)),
                _ => return Err("unknown reply".into()),
            }
        }
    }
    pub fn cmd(&self, c: &str) -> Result<String, String> {
        // flash / erase / format can legitimately take a long time; everything else should answer quickly.
        let secs = if c.starts_with("flash:") || c.starts_with("erase:") || c.starts_with("format:") { 300 }
            else if c.contains("logical-partition") { 120 } else { 30 };
        self.send(c.as_bytes())?;
        let mut info = vec![];
        match self.reply(&mut info, secs)? {
            Reply::Okay(s) => Ok(if s.is_empty() { info.join("\n") } else { s }),
            Reply::Data(_) => Err("unexpected DATA".into()),
        }
    }
    /// Like `cmd`, but returns every INFO/TEXT line followed by the final OKAY text (for manually typed commands).
    pub fn cmd_all(&self, c: &str, secs: u64) -> Result<String, String> {
        self.send(c.as_bytes())?;
        let mut info = vec![];
        match self.reply(&mut info, secs)? {
            Reply::Okay(s) => { if !s.is_empty() { info.push(s); } Ok(info.join("\n")) }
            Reply::Data(_) => Err("cihaz veri istedi (bu komut elle gönderilemez)".into()),
        }
    }
    /// Size the image occupies once flashed: the expanded size for Android-sparse files, the file size otherwise.
    pub fn expanded_size(path: &str) -> Result<u64, String> {
        let mut f = File::open(path).map_err(e)?;
        let len = f.metadata().map_err(e)?.len();
        let mut h = [0u8; 28];
        if f.read_exact(&mut h).is_ok() && u32::from_le_bytes(h[0..4].try_into().unwrap()) == 0xED26FF3A {
            let (blk, total) = (u32::from_le_bytes(h[12..16].try_into().unwrap()) as u64, u32::from_le_bytes(h[16..20].try_into().unwrap()) as u64);
            if blk > 0 && total > 0 { return Ok(blk * total); }
        }
        Ok(len)
    }
    pub fn getvar(&self, v: &str) -> String { self.cmd(&format!("getvar:{v}")).unwrap_or_default() }
    pub fn flash(&self, part: &str, path: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
        let mut f = File::open(path).map_err(e)?;
        let size = f.metadata().map_err(e)?.len();
        progress(0, size);
        self.send(format!("download:{size:08x}").as_bytes())?;
        let mut info = vec![];
        match self.reply(&mut info, 30)? {
            Reply::Data(n) if n as u64 == size => {}
            Reply::Data(n) => return Err(format!("cihaz farklı boyut istedi: {n} / {size}")),
            _ => return Err("device refused download".into()),
        }
        let mut buf = vec![0u8; 1 << 20];
        let mut done = 0u64;
        loop {
            let n = f.read(&mut buf).map_err(e)?;
            if n == 0 { break; }
            self.send_data(&buf[..n], done, size)?;
            done += n as u64;
            progress(done, size);
        }
        self.reply(&mut info, 60)?;
        self.cmd(&format!("flash:{part}")).map(|_| ())
    }

    /// Streams a raw image as several Android-sparse pieces (each <= max bytes), like `fastboot flash` does.
    pub fn flash_split(&self, part: &str, path: &str, max: u64, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
        const BLK: u64 = 4096;
        let mut f = File::open(path).map_err(e)?;
        let size = f.metadata().map_err(e)?.len();
        let mut m = [0u8; 4];
        f.read_exact(&mut m).map_err(e)?;
        if u32::from_le_bytes(m) == 0xED26FF3A { return Err("sparse formatındaki imaj bölünemiyor (önce raw'a çevir)".into()); }
        let total = (size + BLK - 1) / BLK;
        let budget = (max.saturating_sub(28 + 36 + BLK) / BLK).max(1);
        let (mut start, mut done) = (0u64, 0u64);
        let mut buf = vec![0u8; 1 << 20];
        while start < total {
            let n = budget.min(total - start);
            let tail = total - start - n;
            let chunks = 1 + (start > 0) as u32 + (tail > 0) as u32;
            let piece = 28 + 12 * chunks as u64 + n * BLK;
            self.send(format!("download:{piece:08x}").as_bytes())?;
            let mut info = vec![];
            match self.reply(&mut info, 30)? { Reply::Data(x) if x as u64 == piece => {}, _ => return Err("device refused download".into()) }
            let mut h: Vec<u8> = vec![];
            h.extend(0xED26FF3Au32.to_le_bytes()); h.extend(1u16.to_le_bytes()); h.extend(0u16.to_le_bytes());
            h.extend(28u16.to_le_bytes()); h.extend(12u16.to_le_bytes()); h.extend((BLK as u32).to_le_bytes());
            h.extend((total as u32).to_le_bytes()); h.extend(chunks.to_le_bytes()); h.extend(0u32.to_le_bytes());
            let ch = |t: u16, blocks: u64, sz: u64| -> Vec<u8> {
                let mut c = vec![]; c.extend(t.to_le_bytes()); c.extend(0u16.to_le_bytes());
                c.extend((blocks as u32).to_le_bytes()); c.extend((sz as u32).to_le_bytes()); c };
            if start > 0 { h.extend(ch(0xCAC3, start, 12)); }
            h.extend(ch(0xCAC1, n, 12 + n * BLK));
            self.send_data(&h, done, size)?;
            f.seek(SeekFrom::Start(start * BLK)).map_err(e)?;
            let mut left = n * BLK;
            while left > 0 {
                let want = (buf.len() as u64).min(left) as usize;
                let got = f.read(&mut buf[..want]).map_err(e)?;
                if got == 0 { buf[..want].fill(0); }
                let k = if got == 0 { want } else { got };
                self.send_data(&buf[..k], done, size)?;
                left -= k as u64; done += k as u64; progress(done.min(size), size);
            }
            if tail > 0 { self.send_data(&ch(0xCAC3, tail, 12), done, size)?; }
            self.reply(&mut info, 60)?;
            self.cmd(&format!("flash:{part}"))?;
            start += n;
        }
        Ok(())
    }
}
