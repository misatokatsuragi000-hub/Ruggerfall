//! Cursore di lettura little-endian con controllo dei limiti.

use crate::error::{format_err, Error, Result};

pub(crate) struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn at(data: &'a [u8], pos: i64) -> Result<Self> {
        if pos < 0 || pos as usize > data.len() {
            return format_err(format!("posizione fuori dal file: {pos}"));
        }
        Ok(Self {
            data,
            pos: pos as usize,
        })
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| Error::Format("lettura oltre la fine del file".into()))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
}
