//! A clipboard snapshot: the same content in one or more formats, and the
//! wire format used to move it between machines.

use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const TEXT: &str = "text/plain;charset=utf-8";
pub const HTML: &str = "text/html";
pub const PNG: &str = "image/png";


const MAGIC: &[u8; 4] = b"LCL1";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clip {
    pub items: Vec<(String, Vec<u8>)>,
}

impl Clip {
    pub fn is_empty(&self) -> bool {
        self.items.iter().all(|(_, d)| d.is_empty())
    }

    pub fn size(&self) -> usize {
        self.items.iter().map(|(_, d)| d.len()).sum()
    }

    pub fn get(&self, mime: &str) -> Option<&[u8]> {
        self.items.iter().find(|(m, _)| m == mime).map(|(_, d)| d.as_slice())
    }

    /// Identity of the content, used to stop a clip echoing back and forth.
    /// Text wins when present: apps disagree on what else they attach to the
    /// same copy, but the text is what the user copied.
    pub fn hash(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        match self.get(TEXT) {
            Some(text) => h.update(text),
            None => {
                for (m, d) in &self.items {
                    h.update(m.as_bytes());
                    h.update(d);
                }
            }
        }
        h.finalize().into()
    }

    /// One line for the status display.
    pub fn summary(&self) -> String {
        if let Some(t) = self.get(TEXT) {
            let t = String::from_utf8_lossy(t);
            let line: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut s: String = line.chars().take(80).collect();
            if line.chars().count() > 80 {
                s.push('…');
            }
            return s;
        }
        if let Some(png) = self.get(PNG) {
            return format!("image ({} KB)", png.len().div_ceil(1024));
        }
        format!("{} bytes", self.size())
    }

    pub fn kind(&self) -> &'static str {
        if self.get(TEXT).is_some() {
            "text"
        } else if self.get(PNG).is_some() {
            "image"
        } else {
            "other"
        }
    }

    pub async fn write<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        w.write_all(MAGIC).await?;
        w.write_u8(self.items.len() as u8).await?;
        for (mime, data) in &self.items {
            w.write_u8(mime.len() as u8).await?;
            w.write_all(mime.as_bytes()).await?;
            w.write_u32(data.len() as u32).await?;
            w.write_all(data).await?;
        }
        w.flush().await?;
        Ok(())
    }

    pub async fn read<R: AsyncRead + Unpin>(r: &mut R, max: usize) -> Result<Clip> {
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic).await?;
        if &magic != MAGIC {
            bail!("not a lan-clip peer (bad magic)");
        }
        let n = r.read_u8().await?;
        let mut clip = Clip::default();
        let mut total = 0usize;
        for _ in 0..n {
            let mlen = r.read_u8().await? as usize;
            let mut mime = vec![0u8; mlen];
            r.read_exact(&mut mime).await?;
            let len = r.read_u32().await? as usize;
            total += len;
            if total > max {
                bail!("clip larger than max_bytes ({max})");
            }
            // Grow with the bytes that actually arrive, not the length the peer claims.
            let mut data = Vec::new();
            (&mut *r).take(len as u64).read_to_end(&mut data).await?;
            if data.len() != len {
                bail!("clip truncated");
            }
            clip.items.push((String::from_utf8(mime)?, data));
        }
        Ok(clip)
    }
}
