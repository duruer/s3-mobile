//! Minimal explicit binary encoding used by the protocol (no serde: the wire format is documented in
//! docs/porting/multiplayer.md and must stay stable and easy to re-implement, e.g. for a PC bridge).
//!
//! * integers: unsigned LEB128 varints (`u16`/`u32`/`u64`); fixed-size `[u8; N]` raw.
//! * `bytes`: varint length + raw bytes; `string`: `bytes` holding UTF-8.
//! * `Option<T>`: one byte 0/1 then `T`; `Vec<T>`: varint count then items.

use std::fmt;

/// Hard limits so a malicious peer cannot make us allocate unbounded memory.
pub const MAX_BYTES: usize = 64 * 1024;
pub const MAX_STRING: usize = 256;
pub const MAX_ITEMS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    VarintOverflow,
    TooLong { what: &'static str, len: usize },
    BadUtf8,
    BadTag { what: &'static str, tag: u64 },
    TrailingBytes(usize),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated => write!(f, "truncated message"),
            DecodeError::VarintOverflow => write!(f, "varint overflow"),
            DecodeError::TooLong { what, len } => write!(f, "{what} too long ({len})"),
            DecodeError::BadUtf8 => write!(f, "invalid UTF-8"),
            DecodeError::BadTag { what, tag } => write!(f, "unknown {what} tag {tag}"),
            DecodeError::TrailingBytes(n) => write!(f, "{n} trailing bytes"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[derive(Default, Debug, Clone)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self { buf: Vec::with_capacity(64) }
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }
    pub fn var(&mut self, mut v: u64) -> &mut Self {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(b);
                return self;
            }
            self.buf.push(b | 0x80);
        }
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.var(v as u64)
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.var(v as u64)
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.var(v)
    }
    pub fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.var(v.len() as u64);
        self.raw(v)
    }
    pub fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        let b = *self.data.get(self.pos).ok_or(DecodeError::Truncated)?;
        self.pos += 1;
        Ok(b)
    }
    pub fn bool(&mut self) -> Result<bool, DecodeError> {
        Ok(self.u8()? != 0)
    }
    pub fn var(&mut self) -> Result<u64, DecodeError> {
        let mut v: u64 = 0;
        for i in 0..10 {
            let b = self.u8()?;
            let part = (b & 0x7f) as u64;
            if i == 9 && part > 1 {
                return Err(DecodeError::VarintOverflow);
            }
            v |= part << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(DecodeError::VarintOverflow)
    }
    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        u16::try_from(self.var()?).map_err(|_| DecodeError::VarintOverflow)
    }
    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        u32::try_from(self.var()?).map_err(|_| DecodeError::VarintOverflow)
    }
    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        self.var()
    }
    pub fn raw<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        if self.remaining() < N {
            return Err(DecodeError::Truncated);
        }
        let mut out = [0u8; N];
        out.copy_from_slice(&self.data[self.pos..self.pos + N]);
        self.pos += N;
        Ok(out)
    }
    fn len(&mut self, what: &'static str, max: usize) -> Result<usize, DecodeError> {
        let len = self.var()? as usize;
        if len > max {
            return Err(DecodeError::TooLong { what, len });
        }
        Ok(len)
    }
    pub fn bytes(&mut self) -> Result<Vec<u8>, DecodeError> {
        let len = self.len("bytes", MAX_BYTES)?;
        if self.remaining() < len {
            return Err(DecodeError::Truncated);
        }
        let v = self.data[self.pos..self.pos + len].to_vec();
        self.pos += len;
        Ok(v)
    }
    pub fn str(&mut self) -> Result<String, DecodeError> {
        let len = self.len("string", MAX_STRING)?;
        if self.remaining() < len {
            return Err(DecodeError::Truncated);
        }
        let v = std::str::from_utf8(&self.data[self.pos..self.pos + len]).map_err(|_| DecodeError::BadUtf8)?;
        self.pos += len;
        Ok(v.to_owned())
    }
    pub fn count(&mut self) -> Result<usize, DecodeError> {
        self.len("list", MAX_ITEMS)
    }
    pub fn end(&self) -> Result<(), DecodeError> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(DecodeError::TrailingBytes(n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip_and_sizes() {
        for v in [0u64, 1, 127, 128, 300, 16383, 16384, u32::MAX as u64, u64::MAX] {
            let mut w = Writer::new();
            w.var(v);
            let b = w.finish();
            assert_eq!(Reader::new(&b).var().unwrap(), v, "{v}");
        }
        let mut w = Writer::new();
        w.var(127);
        assert_eq!(w.buf.len(), 1);
        let mut w = Writer::new();
        w.var(u64::MAX);
        assert_eq!(w.buf.len(), 10);
    }

    #[test]
    fn rejects_overflow_and_limits() {
        assert_eq!(Reader::new(&[0xff; 11]).var(), Err(DecodeError::VarintOverflow));
        let mut w = Writer::new();
        w.var(70_000);
        assert_eq!(Reader::new(&w.buf).u16(), Err(DecodeError::VarintOverflow));
        let mut w = Writer::new();
        w.var((MAX_BYTES + 1) as u64);
        assert!(matches!(Reader::new(&w.buf).bytes(), Err(DecodeError::TooLong { .. })));
        assert_eq!(Reader::new(&[5, 1, 2]).bytes(), Err(DecodeError::Truncated));
        assert_eq!(Reader::new(&[2, 0xff, 0xfe]).str(), Err(DecodeError::BadUtf8));
    }
}
