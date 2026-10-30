//! Minimal fastboot protocol over a USB fd handed out by Android's UsbManager.
use futures_lite::future::block_on;
use nusb::{transfer::RequestBuffer, Interface};
use std::{fs::File, io::{Read, Seek, SeekFrom}, os::fd::{FromRawFd, OwnedFd}};

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
        block_on(self.iface.bulk_out(self.ep_out, d.to_vec())).into_result().map(|_| ()).map_err(e)
    }
    fn recv(&self) -> Result<Vec<u8>, String> {
        block_on(self.iface.bulk_in(self.ep_in, RequestBuffer::new(4096))).into_result().map_err(e)
    }
    fn reply(&self, info: &mut Vec<String>) -> Result<Reply, String> {
        loop {
            let r = self.recv()?;
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
        self.send(c.as_bytes())?;
        let mut info = vec![];
        match self.reply(&mut info)? {
            Reply::Okay(s) => Ok(if s.is_empty() { info.join("\n") } else { s }),
            Reply::Data(_) => Err("unexpected DATA".into()),
        }
    }
    pub fn getvar(&self, v: &str) -> String { self.cmd(&format!("getvar:{v}")).unwrap_or_default() }
    pub fn flash(&self, part: &str, path: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
        let mut f = File::open(path).map_err(e)?;
        let size = f.metadata().map_err(e)?.len();
        self.send(format!("download:{size:08x}").as_bytes())?;
        let mut info = vec![];
        match self.reply(&mut info)? {
            Reply::Data(n) if n as u64 == size => {}
            _ => return Err("device refused download".into()),
        }
        let mut buf = vec![0u8; 1 << 20];
        let mut done = 0u64;
        loop {
            let n = f.read(&mut buf).map_err(e)?;
            if n == 0 { break; }
            self.send(&buf[..n])?;
            done += n as u64;
            progress(done, size);
        }
        self.reply(&mut info)?;
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
            match self.reply(&mut info)? { Reply::Data(x) if x as u64 == piece => {}, _ => return Err("device refused download".into()) }
            let mut h: Vec<u8> = vec![];
            h.extend(0xED26FF3Au32.to_le_bytes()); h.extend(1u16.to_le_bytes()); h.extend(0u16.to_le_bytes());
            h.extend(28u16.to_le_bytes()); h.extend(12u16.to_le_bytes()); h.extend((BLK as u32).to_le_bytes());
            h.extend((total as u32).to_le_bytes()); h.extend(chunks.to_le_bytes()); h.extend(0u32.to_le_bytes());
            let ch = |t: u16, blocks: u64, sz: u64| -> Vec<u8> {
                let mut c = vec![]; c.extend(t.to_le_bytes()); c.extend(0u16.to_le_bytes());
                c.extend((blocks as u32).to_le_bytes()); c.extend((sz as u32).to_le_bytes()); c };
            if start > 0 { h.extend(ch(0xCAC3, start, 12)); }
            h.extend(ch(0xCAC1, n, 12 + n * BLK));
            self.send(&h)?;
            f.seek(SeekFrom::Start(start * BLK)).map_err(e)?;
            let mut left = n * BLK;
            while left > 0 {
                let want = (buf.len() as u64).min(left) as usize;
                let got = f.read(&mut buf[..want]).map_err(e)?;
                if got == 0 { buf[..want].fill(0); }
                let k = if got == 0 { want } else { got };
                self.send(&buf[..k])?;
                left -= k as u64; done += k as u64; progress(done.min(size), size);
            }
            if tail > 0 { self.send(&ch(0xCAC3, tail, 12))?; }
            self.reply(&mut info)?;
            self.cmd(&format!("flash:{part}"))?;
            start += n;
        }
        Ok(())
    }
}
