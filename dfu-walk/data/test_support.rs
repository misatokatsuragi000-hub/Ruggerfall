//! Helper condivisi dai test per costruire file TEXTURE sintetici.

pub(crate) fn i16le(v: i16) -> [u8; 2] {
    v.to_le_bytes()
}

/// Header di record da 28 byte; i dati partono subito dopo (data_offset = 28).
pub(crate) fn record_header(w: i16, h: i16, compression: u16, frames: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&i16le(0)); // offset x
    v.extend_from_slice(&i16le(0)); // offset y
    v.extend_from_slice(&i16le(w));
    v.extend_from_slice(&i16le(h));
    v.extend_from_slice(&compression.to_le_bytes());
    v.extend_from_slice(&0i32.to_le_bytes()); // record size
    v.extend_from_slice(&28i32.to_le_bytes()); // data offset
    v.extend_from_slice(&i16le(0)); // is normal
    v.extend_from_slice(&frames.to_le_bytes());
    v.extend_from_slice(&i16le(0)); // unknown
    v.extend_from_slice(&i16le(0)); // scale x
    v.extend_from_slice(&i16le(0)); // scale y
    assert_eq!(v.len(), 28);
    v
}

/// Tabella dei record (da 26, 20 byte per voce) seguita dai record.
pub(crate) fn build_file(records: &[Vec<u8>]) -> Vec<u8> {
    let table_end = 26 + 20 * records.len();
    let mut file = vec![0u8; table_end];
    file[0..2].copy_from_slice(&i16le(records.len() as i16));
    let mut pos = table_end;
    for (i, rec) in records.iter().enumerate() {
        let e = 26 + 20 * i;
        file[e + 2..e + 6].copy_from_slice(&(pos as i32).to_le_bytes());
        pos += rec.len();
    }
    for rec in records {
        file.extend_from_slice(rec);
    }
    file
}
