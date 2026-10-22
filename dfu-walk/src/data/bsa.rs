use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct BsaRecord {
    pub name: String,
    pub offset: usize,
    pub size: usize,
    pub resource_id: u32,
}

impl BsaRecord {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn prefix(&self) -> &str {
        if let Some(idx) = self.name.find('.') {
            &self.name[..idx]
        } else {
            self.name.trim_end_matches(|c: char| c.is_ascii_digit())
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn resource_id(&self) -> u32 {
        self.resource_id
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BsaEntry {
    pub offset: usize,
    pub size: usize,
}

pub enum BsaDirectory {
    Named(HashMap<String, BsaEntry>),
    Numeric(HashMap<u32, BsaEntry>),
}

pub struct BsaArchive<'a> {
    pub data: &'a [u8],
    pub directory: BsaDirectory,
}

impl<'a> BsaArchive<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 4 {
            return Err("File BSA troppo piccolo");
        }

        let count = u16::from_le_bytes([data[0], data[1]]) as usize;
        let dir_type = u16::from_le_bytes([data[2], data[3]]);

        match dir_type {
            0x0100 => {
                let entry_size = 18;
                let dir_bytes_len = count * entry_size;
                let dir_offset = data.len().checked_sub(dir_bytes_len)
                .ok_or("Offset directory BSA 0x0100 non valido")?;

                let mut entries = HashMap::with_capacity(count);
                let mut current_offset = 4usize;

                for i in 0..count {
                    let p = dir_offset + i * entry_size;
                    let name_raw = &data[p..p + 14];
                    let rec_size = u32::from_le_bytes(data[p + 14..p + 18].try_into().unwrap()) as usize;

                    let name_len = name_raw.iter().position(|&b| b == 0).unwrap_or(14);
                    let name = String::from_utf8_lossy(&name_raw[..name_len]).trim().to_ascii_uppercase();

                    entries.insert(name, BsaEntry {
                        offset: current_offset,
                        size: rec_size,
                    });
                    current_offset += rec_size;
                }

                Ok(Self { data, directory: BsaDirectory::Named(entries) })
            }
            0x0200 => {
                let entry_size = 8;
                let dir_bytes_len = count * entry_size;
                let dir_offset = data.len().checked_sub(dir_bytes_len)
                .ok_or("Offset directory BSA 0x0200 non valido")?;

                let mut entries = HashMap::with_capacity(count);
                let mut current_offset = 4usize;

                for i in 0..count {
                    let p = dir_offset + i * entry_size;
                    let id = u32::from_le_bytes(data[p..p + 4].try_into().unwrap());
                    let rec_size = u32::from_le_bytes(data[p + 4..p + 8].try_into().unwrap()) as usize;

                    entries.insert(id, BsaEntry {
                        offset: current_offset,
                        size: rec_size,
                    });
                    current_offset += rec_size;
                }

                Ok(Self { data, directory: BsaDirectory::Numeric(entries) })
            }
            _ => Err("Tipo directory BSA non supportato"),
        }
    }

    pub fn get_by_name(&self, name: &str) -> Option<&'a [u8]> {
        if let BsaDirectory::Named(ref map) = self.directory {
            let key = name.to_ascii_uppercase();
            let entry = map.get(&key)?;
            Some(&self.data[entry.offset..entry.offset + entry.size])
        } else {
            None
        }
    }

    pub fn get_by_id(&self, id: u32) -> Option<&'a [u8]> {
        if let BsaDirectory::Numeric(ref map) = self.directory {
            let entry = map.get(&id)?;
            Some(&self.data[entry.offset..entry.offset + entry.size])
        } else {
            None
        }
    }
}

pub struct BsaFile {
    pub data: Vec<u8>,
    pub records_list: Vec<BsaRecord>,
    pub by_name: HashMap<String, usize>,
}

impl BsaFile {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, std::io::Error> {
        let mut f = File::open(path)?;
        let mut data = Vec::new();
        f.read_to_end(&mut data)?;
        Self::from_bytes(data).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Self, &'static str> {
        if data.len() < 4 {
            return Err("File BSA troppo piccolo");
        }
        let count = u16::from_le_bytes([data[0], data[1]]) as usize;
        let entry_size = 18;
        let dir_bytes_len = count * entry_size;
        let dir_offset = data.len().checked_sub(dir_bytes_len).ok_or("Offset errato")?;

        let mut records_list = Vec::with_capacity(count);
        let mut by_name = HashMap::with_capacity(count);
        let mut current_offset = 4usize;

        for i in 0..count {
            let p = dir_offset + i * entry_size;
            let name_raw = &data[p..p + 14];
            let rec_size = u32::from_le_bytes(data[p + 14..p + 18].try_into().unwrap()) as usize;
            let name_len = name_raw.iter().position(|&b| b == 0).unwrap_or(14);
            let name = String::from_utf8_lossy(&name_raw[..name_len]).trim().to_ascii_uppercase();

            let resource_id = if let Some(dot) = name.find('.') {
                name[dot + 1..].trim().parse::<u32>().unwrap_or(0)
            } else {
                0
            };

            by_name.insert(name.clone(), records_list.len());
            records_list.push(BsaRecord {
                name,
                offset: current_offset,
                size: rec_size,
                resource_id,
            });
            current_offset += rec_size;
        }

        Ok(Self { data, records_list, by_name })
    }

    pub fn records(&self) -> &[BsaRecord] {
        &self.records_list
    }

    pub fn block(&self, record: &BsaRecord) -> Result<&[u8], std::io::Error> {
        if record.offset + record.size <= self.data.len() {
            Ok(&self.data[record.offset..record.offset + record.size])
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Offset blocco BSA fuori limite",
            ))
        }
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        let idx = *self.by_name.get(&name.to_ascii_uppercase())?;
        let rec = &self.records_list[idx];
        Some(&self.data[rec.offset..rec.offset + rec.size])
    }

    pub fn blocks_with_prefix<'b>(&'b self, prefix: &str) -> Result<Vec<(&'b str, &'b [u8])>, std::io::Error> {
        let pref = prefix.to_ascii_uppercase();
        let mut results = Vec::new();
        for rec in &self.records_list {
            if rec.prefix() == pref || rec.name.starts_with(&pref) {
                let slice = &self.data[rec.offset..rec.offset + rec.size];
                results.push((rec.name.as_str(), slice));
            }
        }
        Ok(results)
    }
}
