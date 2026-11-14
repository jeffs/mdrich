//! Chromium's `org.chromium.web-custom-data` pasteboard flavor.
//!
//! A web page that calls `DataTransfer.setData` with a type Chromium has no
//! native flavor for gets its entries serialized here as a Pickle: a `u32`
//! payload length, a `u32` entry count, then alternating key and value
//! strings. Each string is a `u32` count of UTF-16 code units followed by the
//! code units themselves, and every field is padded up to a four-byte
//! boundary.

use std::collections::HashMap;

/// The custom `DataTransfer` types a page offered, keyed by type name.
pub type CustomData = HashMap<String, String>;

pub fn parse(bytes: &[u8]) -> Option<CustomData> {
    let mut reader = Reader::new(bytes);
    let _payload_len = reader.u32()?;
    let entries = reader.u32()?;
    let mut data = CustomData::new();
    for _ in 0..entries {
        let key = reader.utf16()?;
        let value = reader.utf16()?;
        data.insert(key, value);
    }
    Some(data)
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(len)?;
        let field = self.bytes.get(self.offset..end)?;
        self.offset = end.checked_add(3)? & !3;
        Some(field)
    }

    fn u32(&mut self) -> Option<u32> {
        let field = self.take(size_of::<u32>())?;
        Some(u32::from_le_bytes(field.try_into().ok()?))
    }

    fn utf16(&mut self) -> Option<String> {
        let len = usize::try_from(self.u32()?).ok()?;
        let field = self.take(len.checked_mul(2)?)?;
        let units: Vec<u16> = field
            .as_chunks::<2>().0.iter()
            .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
            .collect();
        String::from_utf16(&units).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pickle(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
        for (key, value) in entries {
            for field in [key, value] {
                let units: Vec<u16> = field.encode_utf16().collect();
                payload.extend_from_slice(&u32::try_from(units.len()).unwrap().to_le_bytes());
                for unit in units {
                    payload.extend_from_slice(&unit.to_le_bytes());
                }
                payload.resize(payload.len().next_multiple_of(4), 0);
            }
        }
        let mut bytes = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
        bytes.extend_from_slice(&payload);
        bytes
    }

    #[test]
    fn reads_entries() {
        let data = parse(&pickle(&[("text/plain", "hi"), ("slack/texty", "{}")])).unwrap();
        assert_eq!(data["text/plain"], "hi");
        assert_eq!(data["slack/texty"], "{}");
    }

    #[test]
    fn realigns_after_odd_length_strings() {
        let data = parse(&pickle(&[("odd", "abc"), ("k", "v")])).unwrap();
        assert_eq!(data["odd"], "abc");
        assert_eq!(data["k"], "v");
    }

    #[test]
    fn rejects_truncated_input() {
        let bytes = pickle(&[("text/plain", "hi")]);
        assert!(parse(&bytes[..bytes.len() - 4]).is_none());
    }

    #[test]
    fn rejects_absurd_entry_count() {
        let mut bytes = pickle(&[("text/plain", "hi")]);
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&bytes).is_none());
    }
}
