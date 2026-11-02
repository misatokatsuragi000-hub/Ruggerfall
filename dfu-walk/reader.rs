//! Minimal bounds-checked byte reader with switchable endianness.

use crate::error::{Error, Result};

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    le: bool,
}

macro_rules! num {
    ($name:ident, $t:ty, $n:expr) => {
        pub fn $name(&mut self) -> Result<$t> {
            let a: [u8; $n] = self.take($n)?.try_into().unwrap();
            Ok(if self.le { <$t>::from_le_bytes(a) } else { <$t>::from_be_bytes(a) })
        }
    };
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8], little_endian: bool) -> Self {
        Self { buf, pos: 0, le: little_endian }
    }

    pub fn set_little_endian(&mut self, le: bool) {
        self.le = le;
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or(Error::Eof { offset: self.pos, wanted: n })?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    pub fn align(&mut self, n: usize) -> Result<()> {
        let rem = self.pos % n;
        if rem != 0 {
            self.skip(n - rem)?;
        }
        Ok(())
    }

    num!(u8, u8, 1);
    num!(i16, i16, 2);
    num!(u16, u16, 2);
    num!(i32, i32, 4);
    num!(u32, u32, 4);
    num!(i64, i64, 8);
    num!(u64, u64, 8);
    num!(f32, f32, 4);
    num!(f64, f64, 8);

    /// NUL-terminated UTF-8 string.
    pub fn cstr(&mut self) -> Result<String> {
        let rest = &self.buf[self.pos..];
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(Error::Eof { offset: self.pos, wanted: rest.len() + 1 })?;
        let s = String::from_utf8_lossy(&rest[..len]).into_owned();
        self.pos += len + 1;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endianness_and_align() {
        let data = [0, 0, 0, 1, 1, 0, 0, 0, 9];
        let mut r = Reader::new(&data, false);
        assert_eq!(r.u32().unwrap(), 1);
        r.set_little_endian(true);
        assert_eq!(r.u32().unwrap(), 1);
        r.align(4).unwrap(); // already aligned at 8: no-op
        assert_eq!(r.u8().unwrap(), 9);
        assert!(r.u8().is_err());
    }

    #[test]
    fn align_skips_padding() {
        let mut r = Reader::new(&[1, 2, 3, 4, 5], true);
        r.skip(1).unwrap();
        r.align(4).unwrap();
        assert_eq!(r.pos(), 4);
    }

    #[test]
    fn eof_is_an_error_not_a_panic() {
        let mut r = Reader::new(&[1, 2], true);
        assert!(r.u32().is_err());
        assert_eq!(r.u16().unwrap(), 0x0201);
    }

    #[test]
    fn cstr_reads_until_nul() {
        let mut r = Reader::new(b"abc\0def", true);
        assert_eq!(r.cstr().unwrap(), "abc");
        assert!(r.cstr().is_err()); // no terminator
    }
}
