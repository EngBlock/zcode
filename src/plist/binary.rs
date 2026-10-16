//! The binary property list format (`bplist00`): a table of objects (each a
//! marker byte, then its data or the indices of the objects it contains), an
//! offset table, and a 32-byte trailer.

use anyhow::{Result, anyhow, bail};

use super::{Entry, Node, Value};

struct Reader<'a> {
    bytes: &'a [u8],
    offsets: Vec<usize>,
    ref_size: usize,
}

fn read_uint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |acc, &b| acc << 8 | b as u64)
}

impl Reader<'_> {
    fn slice(&self, start: usize, len: usize) -> Result<&[u8]> {
        self.bytes.get(start..start.checked_add(len).ok_or_else(|| anyhow!("bad length"))?).ok_or_else(|| anyhow!("truncated object at byte {start}"))
    }

    /// An object's length: the marker's low nibble, or an integer after it.
    fn length(&self, marker: u8, at: usize) -> Result<(usize, usize)> {
        let low = (marker & 0x0f) as usize;
        if low != 0x0f {
            return Ok((low, at + 1));
        }
        let int_marker = *self.bytes.get(at + 1).ok_or_else(|| anyhow!("truncated length"))?;
        if int_marker & 0xf0 != 0x10 {
            bail!("bad length marker at byte {at}");
        }
        let size = 1usize << (int_marker & 0x0f);
        let len = read_uint(self.slice(at + 2, size)?) as usize;
        Ok((len, at + 2 + size))
    }

    fn refs(&self, at: usize, count: usize) -> Result<Vec<usize>> {
        let data = self.slice(at, count.checked_mul(self.ref_size).ok_or_else(|| anyhow!("bad count"))?)?;
        Ok(data.chunks(self.ref_size).map(|c| read_uint(c) as usize).collect())
    }

    fn object(&self, index: usize, depth: usize) -> Result<Value> {
        if depth > 512 {
            bail!("property list nests too deeply");
        }
        let at = *self.offsets.get(index).ok_or_else(|| anyhow!("object {index} out of range"))?;
        let marker = *self.bytes.get(at).ok_or_else(|| anyhow!("offset out of range"))?;
        Ok(match marker >> 4 {
            0x0 => match marker {
                0x08 => Value::Bool(false),
                0x09 => Value::Bool(true),
                // Null and fill bytes have no XML form.
                _ => bail!("unsupported object 0x{marker:02x}"),
            },
            0x1 => {
                let size = 1usize << (marker & 0x0f);
                let data = self.slice(at + 1, size)?;
                Value::Integer(match size {
                    1 | 2 | 4 => read_uint(data) as i128,
                    8 => read_uint(data) as i64 as i128,
                    // 16-byte integers hold unsigned 64-bit values.
                    16 => read_uint(&data[8..]) as i128,
                    _ => bail!("bad integer size {size}"),
                })
            }
            0x2 => {
                let size = 1usize << (marker & 0x0f);
                let data = self.slice(at + 1, size)?;
                Value::Real(match size {
                    4 => f32::from_be_bytes(data.try_into()?) as f64,
                    8 => f64::from_be_bytes(data.try_into()?),
                    _ => bail!("bad real size {size}"),
                })
            }
            0x3 if marker == 0x33 => Value::Date(f64::from_be_bytes(self.slice(at + 1, 8)?.try_into()?)),
            0x4 => {
                let (len, start) = self.length(marker, at)?;
                Value::Data(self.slice(start, len)?.to_vec())
            }
            0x5 => {
                let (len, start) = self.length(marker, at)?;
                Value::String(self.slice(start, len)?.iter().map(|&b| b as char).collect())
            }
            0x6 => {
                let (len, start) = self.length(marker, at)?;
                let units: Vec<u16> = self.slice(start, len * 2)?.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                Value::String(String::from_utf16_lossy(&units))
            }
            // Keyed archives' UIDs read as CoreFoundation shows them in XML.
            0x8 => {
                let uid = read_uint(self.slice(at + 1, (marker & 0x0f) as usize + 1)?);
                Value::Dict(vec![Entry { key: "CF$UID".into(), key_span: 0..0, value: Node::new(Value::Integer(uid as i128)) }])
            }
            0xa => {
                let (len, start) = self.length(marker, at)?;
                let items = self.refs(start, len)?;
                Value::Array(items.into_iter().map(|i| self.object(i, depth + 1).map(Node::new)).collect::<Result<_>>()?)
            }
            0xd => {
                let (len, start) = self.length(marker, at)?;
                let keys = self.refs(start, len)?;
                let values = self.refs(start + len * self.ref_size, len)?;
                let mut entries = Vec::with_capacity(len);
                for (k, v) in keys.into_iter().zip(values) {
                    let Value::String(key) = self.object(k, depth + 1)? else { bail!("dictionary key isn’t a string") };
                    entries.push(Entry { key, key_span: 0..0, value: Node::new(self.object(v, depth + 1)?) });
                }
                Value::Dict(entries)
            }
            _ => bail!("unsupported object 0x{marker:02x}"),
        })
    }
}

pub fn read(bytes: &[u8]) -> Result<Value> {
    if !super::is_binary(bytes) || bytes.len() < 8 + 32 {
        bail!("not a binary property list");
    }
    let trailer = &bytes[bytes.len() - 32..];
    let offset_size = trailer[6] as usize;
    let ref_size = trailer[7] as usize;
    let count = read_uint(&trailer[8..16]) as usize;
    let top = read_uint(&trailer[16..24]) as usize;
    let table = read_uint(&trailer[24..32]) as usize;
    if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) || count == 0 {
        bail!("bad binary property list trailer");
    }
    let table_bytes = bytes
        .get(table..table.checked_add(count.checked_mul(offset_size).ok_or_else(|| anyhow!("bad count"))?).ok_or_else(|| anyhow!("bad table"))?)
        .ok_or_else(|| anyhow!("truncated offset table"))?;
    let offsets = table_bytes.chunks(offset_size).map(|c| read_uint(c) as usize).collect();
    Reader { bytes, offsets, ref_size }.object(top, 0)
}

// ---------------------------------------------------------------------------
// Writing

struct Writer {
    /// Each object's encoding, with object references as indices to fix up.
    objects: Vec<Object>,
}

enum Object {
    Bytes(Vec<u8>),
    /// A container: its marker and length header, then references.
    Refs(Vec<u8>, Vec<usize>),
}

fn header(kind: u8, len: usize) -> Vec<u8> {
    if len < 15 {
        return vec![kind << 4 | len as u8];
    }
    let mut out = vec![kind << 4 | 0x0f];
    out.extend(int_bytes(len as i128));
    out
}

fn int_bytes(i: i128) -> Vec<u8> {
    if i < 0 || i > i64::MAX as i128 {
        if i < 0 {
            let mut out = vec![0x13];
            out.extend((i as i64).to_be_bytes());
            return out;
        }
        let mut out = vec![0x14];
        out.extend([0u8; 8]);
        out.extend((i as u64).to_be_bytes());
        return out;
    }
    let v = i as u64;
    if v <= u8::MAX as u64 {
        vec![0x10, v as u8]
    } else if v <= u16::MAX as u64 {
        let mut out = vec![0x11];
        out.extend((v as u16).to_be_bytes());
        out
    } else if v <= u32::MAX as u64 {
        let mut out = vec![0x12];
        out.extend((v as u32).to_be_bytes());
        out
    } else {
        let mut out = vec![0x13];
        out.extend(v.to_be_bytes());
        out
    }
}

fn string_bytes(s: &str) -> Vec<u8> {
    if s.is_ascii() {
        let mut out = header(0x5, s.len());
        out.extend(s.as_bytes());
        return out;
    }
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut out = header(0x6, units.len());
    for unit in units {
        out.extend(unit.to_be_bytes());
    }
    out
}

impl Writer {
    fn add(&mut self, value: &Value) -> usize {
        let index = self.objects.len();
        // Reserve the slot so containers come before their contents.
        self.objects.push(Object::Bytes(vec![]));
        let object = match value {
            Value::Bool(b) => Object::Bytes(vec![if *b { 0x09 } else { 0x08 }]),
            Value::Integer(i) => Object::Bytes(int_bytes(*i)),
            Value::Real(r) => {
                let mut out = vec![0x23];
                out.extend(r.to_be_bytes());
                Object::Bytes(out)
            }
            Value::Date(d) => {
                let mut out = vec![0x33];
                out.extend(d.to_be_bytes());
                Object::Bytes(out)
            }
            Value::Data(bytes) => {
                let mut out = header(0x4, bytes.len());
                out.extend(bytes);
                Object::Bytes(out)
            }
            Value::String(s) => Object::Bytes(string_bytes(s)),
            Value::Array(items) => {
                let refs = items.iter().map(|n| self.add(&n.value)).collect();
                Object::Refs(header(0xa, items.len()), refs)
            }
            Value::Dict(entries) => {
                let mut refs: Vec<usize> = entries.iter().map(|e| self.add(&Value::String(e.key.clone()))).collect();
                refs.extend(entries.iter().map(|e| self.add(&e.value.value)));
                Object::Refs(header(0xd, entries.len()), refs)
            }
        };
        self.objects[index] = object;
        index
    }
}

fn size_for(max: usize) -> usize {
    match max {
        0..=0xff => 1,
        0x100..=0xffff => 2,
        0x1_0000..=0xffff_ffff => 4,
        _ => 8,
    }
}

fn push_uint(out: &mut Vec<u8>, value: usize, size: usize) {
    out.extend(&(value as u64).to_be_bytes()[8 - size..]);
}

pub fn write(value: &Value) -> Vec<u8> {
    let mut writer = Writer { objects: vec![] };
    writer.add(value);
    let count = writer.objects.len();
    let ref_size = size_for(count);
    let mut out = b"bplist00".to_vec();
    let mut offsets = Vec::with_capacity(count);
    for object in &writer.objects {
        offsets.push(out.len());
        match object {
            Object::Bytes(bytes) => out.extend(bytes),
            Object::Refs(head, refs) => {
                out.extend(head);
                for &r in refs {
                    push_uint(&mut out, r, ref_size);
                }
            }
        }
    }
    let table = out.len();
    let offset_size = size_for(table);
    for offset in offsets {
        push_uint(&mut out, offset, offset_size);
    }
    out.extend([0u8; 6]);
    out.push(offset_size as u8);
    out.push(ref_size as u8);
    out.extend((count as u64).to_be_bytes());
    out.extend(0u64.to_be_bytes());
    out.extend((table as u64).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        let entry = |key: &str, value: Value| Entry { key: key.into(), key_span: 0..0, value: Node::new(value) };
        Value::Dict(vec![
            entry("zeta", Value::String("last key first".into())),
            entry("unicode", Value::String("héllo 😀".into())),
            entry("long", Value::String("x".repeat(300))),
            entry("ints", Value::Array([0i128, 255, 256, 65_536, -1, i64::MAX as i128, u64::MAX as i128].map(|i| Node::new(Value::Integer(i))).to_vec())),
            entry("real", Value::Real(0.1)),
            entry("date", Value::Date(725_846_400.)),
            entry("data", Value::Data((0..40).collect())),
            entry("flags", Value::Array(vec![Node::new(Value::Bool(true)), Node::new(Value::Bool(false))])),
            entry("empty", Value::Dict(vec![])),
        ])
    }

    #[test]
    fn round_trips() {
        let value = sample();
        assert_eq!(read(&write(&value)).unwrap(), value);
    }

    #[test]
    fn many_objects_need_wider_references() {
        let items = (0..70_000).map(|i| Node::new(Value::Integer(i))).collect();
        let value = Value::Array(items);
        assert_eq!(read(&write(&value)).unwrap(), value);
    }

    #[test]
    fn reads_what_plutil_writes() {
        let dir = std::env::temp_dir().join(format!("zcode-bplist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let xml = dir.join("in.plist");
        std::fs::write(&xml, super::super::to_xml(&sample())).unwrap();
        let binary = dir.join("out.plist");
        let status = std::process::Command::new("/usr/bin/plutil").args(["-convert", "binary1", "-o"]).arg(&binary).arg(&xml).status();
        if status.is_ok_and(|s| s.success()) {
            let read_back = read(&std::fs::read(&binary).unwrap()).unwrap();
            // plutil sorts keys; compare as sets of entries.
            let (Value::Dict(mut a), Value::Dict(mut b)) = (read_back, sample()) else { panic!() };
            a.sort_by(|x, y| x.key.cmp(&y.key));
            b.sort_by(|x, y| x.key.cmp(&y.key));
            assert_eq!(a, b);
            // And plutil reads what zcode writes.
            std::fs::write(&binary, write(&sample())).unwrap();
            let lint = std::process::Command::new("/usr/bin/plutil").arg("-lint").arg(&binary).output().unwrap();
            assert!(lint.status.success(), "{}", String::from_utf8_lossy(&lint.stdout));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_garbage() {
        assert!(read(b"bplist00").is_err());
        assert!(read(&[b'b', b'p', b'l', b'i', b's', b't', b'0', b'0'].repeat(10)).is_err());
    }
}
