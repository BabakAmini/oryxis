//! Random-access source abstraction + a caching `Read + Seek` adapter.
//!
//! The zip central directory lives at the END of the file, so browsing
//! an archive needs positioned reads, not a stream. [`RangedSource`] is
//! the minimal sync contract; the app implements it over SFTP ranged
//! reads (bridging to async on a blocking thread) and over local files.
//! [`CachedRangeReader`] then adapts any source to the `Read + Seek`
//! interface the `zip` crate expects, fetching fixed-size chunks and
//! keeping a small FIFO cache so the parser's seek-heavy access pattern
//! doesn't re-fetch the same region over the network.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Seek, SeekFrom};

/// A source of positioned reads. `read_at` may return fewer bytes than
/// requested only at end-of-file; intermediate short reads are looped
/// by the caller.
pub trait RangedSource: Send {
    /// Total size in bytes. Called once at adapter construction.
    fn len(&mut self) -> io::Result<u64>;

    fn is_empty(&mut self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Read into `buf` starting at absolute `offset`. Returns the byte
    /// count read; `0` only at/after end-of-file.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
}

impl RangedSource for std::fs::File {
    fn len(&mut self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.seek(SeekFrom::Start(offset))?;
        self.read(buf)
    }
}

/// Object-safe delegation so callers can erase the concrete source
/// (local file vs network bridge) behind one reader type.
impl RangedSource for Box<dyn RangedSource> {
    fn len(&mut self) -> io::Result<u64> {
        (**self).len()
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read_at(offset, buf)
    }
}

/// Test / in-memory convenience.
impl RangedSource for io::Cursor<Vec<u8>> {
    fn len(&mut self) -> io::Result<u64> {
        Ok(self.get_ref().len() as u64)
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.set_position(offset);
        self.read(buf)
    }
}

/// `Read + Seek` over a [`RangedSource`] with fixed-size chunk caching.
pub struct CachedRangeReader<S: RangedSource> {
    src: S,
    len: u64,
    pos: u64,
    chunk_size: u64,
    max_chunks: usize,
    chunks: HashMap<u64, Vec<u8>>,
    order: VecDeque<u64>,
}

impl<S: RangedSource> CachedRangeReader<S> {
    /// `chunk_size` trades round trips against over-fetch; the SFTP
    /// bridge wants something a few times the 255 KiB per-request
    /// ceiling, a local file is happy with anything.
    pub fn new(mut src: S, chunk_size: u64, max_chunks: usize) -> io::Result<Self> {
        assert!(chunk_size > 0 && max_chunks > 0);
        let len = src.len()?;
        Ok(Self {
            src,
            len,
            pos: 0,
            chunk_size,
            max_chunks,
            chunks: HashMap::new(),
            order: VecDeque::new(),
        })
    }

    pub fn source_len(&self) -> u64 {
        self.len
    }

    /// Fetch (or recall) the chunk with index `idx`, returning a
    /// reference to its bytes.
    fn chunk(&mut self, idx: u64) -> io::Result<&[u8]> {
        if !self.chunks.contains_key(&idx) {
            let offset = idx * self.chunk_size;
            let want = (self.len - offset).min(self.chunk_size) as usize;
            let mut buf = vec![0u8; want];
            let mut filled = 0;
            while filled < want {
                let n = self.src.read_at(offset + filled as u64, &mut buf[filled..])?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!("short read at {} (wanted {want}, got {filled})", offset),
                    ));
                }
                filled += n;
            }
            if self.order.len() >= self.max_chunks
                && let Some(evict) = self.order.pop_front()
            {
                self.chunks.remove(&evict);
            }
            self.order.push_back(idx);
            self.chunks.insert(idx, buf);
        }
        Ok(self.chunks.get(&idx).expect("just inserted"))
    }
}

impl<S: RangedSource> Read for CachedRangeReader<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.pos >= self.len {
            return Ok(0);
        }
        let mut written = 0;
        while written < buf.len() && self.pos < self.len {
            let idx = self.pos / self.chunk_size;
            let within = (self.pos % self.chunk_size) as usize;
            let chunk = self.chunk(idx)?;
            let take = (chunk.len() - within).min(buf.len() - written);
            buf[written..written + take].copy_from_slice(&chunk[within..within + take]);
            written += take;
            self.pos += take as u64;
        }
        Ok(written)
    }
}

impl<S: RangedSource> Seek for CachedRangeReader<S> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target: i128 = match pos {
            SeekFrom::Start(o) => o as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = target as u64;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn reads_match_reference_across_chunk_boundaries() {
        let bytes = data(10_000);
        let mut r = CachedRangeReader::new(io::Cursor::new(bytes.clone()), golden_chunk(), 4).unwrap();
        // Straddle chunk boundaries on purpose.
        for (start, len) in [(0usize, 10), (250, 300), (999, 1), (9_990, 100), (5_000, 5_000)] {
            r.seek(SeekFrom::Start(start as u64)).unwrap();
            let mut buf = vec![0u8; len];
            let n = r.read(&mut buf).unwrap();
            let expect = &bytes[start..(start + len).min(bytes.len())];
            assert_eq!(&buf[..n], expect, "range {start}+{len}");
        }
    }

    fn golden_chunk() -> u64 {
        256
    }

    #[test]
    fn seek_end_and_eof() {
        let bytes = data(1000);
        let mut r = CachedRangeReader::new(io::Cursor::new(bytes.clone()), 128, 2).unwrap();
        let p = r.seek(SeekFrom::End(-22)).unwrap();
        assert_eq!(p, 978);
        let mut buf = [0u8; 64];
        assert_eq!(r.read(&mut buf).unwrap(), 22);
        assert_eq!(r.read(&mut buf).unwrap(), 0);
        assert!(r.seek(SeekFrom::Current(-2000)).is_err());
    }

    #[test]
    fn cache_eviction_stays_correct() {
        let bytes = data(4096);
        // Tiny cache: 2 chunks of 512; walk forward then re-read start.
        let mut r = CachedRangeReader::new(io::Cursor::new(bytes.clone()), 512, 2).unwrap();
        let mut all = Vec::new();
        r.read_to_end(&mut all).unwrap();
        assert_eq!(all, bytes);
        r.seek(SeekFrom::Start(0)).unwrap();
        let mut head = [0u8; 16];
        r.read_exact(&mut head).unwrap();
        assert_eq!(&head[..], &bytes[..16]);
    }
}
