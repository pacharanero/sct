// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! On-disk container format for the single-file `snomed.fst` artefact.
//!
//! The artefact bundles several logical sections (two FSTs plus their posting
//! lists and display side-tables) into one mmap-able file with a table of
//! contents at the end - zip/parquet style:
//!
//! ```text
//! magic "SCTFST\0\0" (8) | u32 format_version
//! section bytes (concatenated, in write order)
//! TOC: u32 count, then per entry: u8 name_len, name, u64 offset, u64 len
//! footer: u64 toc_offset (the final 8 bytes of the file)
//! ```
//!
//! A reader mmaps the whole file, reads the final 8 bytes to locate the TOC,
//! then hands each section out as a zero-copy byte range.
//!
//! It also defines the packing of the FST `u64` value:
//! `(semantic_tag_id << 56) | posting_offset`.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::io::{self, Write};
use std::ops::Range;

/// Magic at the very start of the file. Bump [`FORMAT_VERSION`] for incompatible
/// changes to the byte layout.
pub const MAGIC: &[u8; 8] = b"SCTFST\0\0";

/// Container byte-layout version (independent of the NDJSON schema version and
/// the `sct` crate version, both of which are recorded in the provenance
/// section instead).
///
/// v2: posting lists are delta + unsigned-varint encoded (was raw `[u32 len][u64]*`).
pub const FORMAT_VERSION: u32 = 2;

// Canonical section names.
pub const SEC_DESCRIPTIONS: &str = "descriptions"; // fst: normalised term -> packed value
pub const SEC_POSTINGS: &str = "postings"; // uvarint len, then delta-uvarint SCTID lists
pub const SEC_WORDS: &str = "words"; // fst: token -> packed value
pub const SEC_WORD_POSTINGS: &str = "word_postings"; // uvarint len, then delta-uvarint SCTID lists
pub const SEC_TERMS_INDEX: &str = "terms_index"; // u32 count, then (u64 sctid, u32 off, u32 len)*
pub const SEC_TERMS_TEXT: &str = "terms_text"; // concatenated original-case preferred terms
pub const SEC_TAG_TABLE: &str = "tag_table"; // JSON array of tag strings (index 0 = "")
pub const SEC_PROVENANCE: &str = "provenance"; // JSON provenance object, or empty

const TAG_SHIFT: u64 = 56;
const OFFSET_MASK: u64 = (1u64 << TAG_SHIFT) - 1;

/// Pack a one-byte semantic-tag id and a 56-bit posting offset into the FST value.
pub fn pack(tag_id: u8, offset: u64) -> u64 {
    debug_assert!(offset <= OFFSET_MASK, "posting offset exceeds 56 bits");
    ((tag_id as u64) << TAG_SHIFT) | (offset & OFFSET_MASK)
}

/// Inverse of [`pack`]: `(tag_id, offset)`.
pub fn unpack(value: u64) -> (u8, u64) {
    ((value >> TAG_SHIFT) as u8, value & OFFSET_MASK)
}

/// Append an unsigned LEB128 varint. Used for delta-encoded posting lists: small
/// deltas (the common case for dense, sorted SCTID runs) cost one or two bytes
/// instead of a flat eight.
pub fn write_uvarint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

/// Decode an unsigned LEB128 varint from `data` at `*pos`, advancing `*pos`.
/// Returns `None` on truncation or on a value that would overflow `u64`.
pub fn read_uvarint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    loop {
        let byte = *data.get(*pos)?;
        *pos += 1;
        if shift >= 64 {
            return None;
        }
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
    }
}

/// A named blob to be written into the container.
pub struct Section<'a> {
    pub name: &'static str,
    pub bytes: &'a [u8],
}

/// Stream a set of sections into `w` as a complete container file.
pub fn write_container<W: Write>(w: &mut W, sections: &[Section<'_>]) -> io::Result<()> {
    let mut offset: u64 = 0;
    w.write_all(MAGIC)?;
    offset += MAGIC.len() as u64;
    w.write_all(&FORMAT_VERSION.to_le_bytes())?;
    offset += 4;

    let mut toc: Vec<(&str, u64, u64)> = Vec::with_capacity(sections.len());
    for s in sections {
        let start = offset;
        w.write_all(s.bytes)?;
        offset += s.bytes.len() as u64;
        toc.push((s.name, start, s.bytes.len() as u64));
    }

    let toc_offset = offset;
    w.write_all(&(toc.len() as u32).to_le_bytes())?;
    for (name, start, len) in &toc {
        let nb = name.as_bytes();
        w.write_all(&[nb.len() as u8])?;
        w.write_all(nb)?;
        w.write_all(&start.to_le_bytes())?;
        w.write_all(&len.to_le_bytes())?;
    }
    // Footer: the TOC offset as the final 8 bytes, so a reader can find it from
    // the end without scanning.
    w.write_all(&toc_offset.to_le_bytes())?;
    Ok(())
}

/// Parsed table of contents: section name -> byte range within the file.
pub struct Toc {
    sections: HashMap<String, Range<usize>>,
}

impl Toc {
    /// Parse the TOC from a full container byte slice (typically the mmap).
    pub fn parse(bytes: &[u8]) -> Result<Toc> {
        if bytes.len() < MAGIC.len() + 4 + 8 {
            bail!("file too small to be a snomed.fst container");
        }
        if &bytes[..MAGIC.len()] != MAGIC {
            bail!("not a snomed.fst container (bad magic)");
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if version != FORMAT_VERSION {
            bail!(
                "unsupported snomed.fst format version {version} (this build understands {FORMAT_VERSION})"
            );
        }

        let toc_offset = u64::from_le_bytes(bytes[bytes.len() - 8..].try_into().unwrap()) as usize;
        if toc_offset + 4 > bytes.len() {
            bail!("corrupt container: TOC offset out of range");
        }
        let mut p = toc_offset;
        let count = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap()) as usize;
        p += 4;

        let mut sections = HashMap::with_capacity(count);
        for _ in 0..count {
            if p + 1 > bytes.len() {
                bail!("corrupt container: truncated TOC");
            }
            let name_len = bytes[p] as usize;
            p += 1;
            if p + name_len + 16 > bytes.len() {
                bail!("corrupt container: truncated TOC entry");
            }
            let name = std::str::from_utf8(&bytes[p..p + name_len])
                .context("TOC section name is not UTF-8")?
                .to_string();
            p += name_len;
            let start = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap()) as usize;
            p += 8;
            let len = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap()) as usize;
            p += 8;
            if start + len > bytes.len() {
                bail!("corrupt container: section {name} runs past end of file");
            }
            sections.insert(name, start..start + len);
        }
        Ok(Toc { sections })
    }

    /// Byte range of a required section, or an error naming the missing section.
    pub fn require(&self, name: &str) -> Result<Range<usize>> {
        self.sections
            .get(name)
            .cloned()
            .with_context(|| format!("snomed.fst container is missing the '{name}' section"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_roundtrip() {
        for &(tag, off) in &[(0u8, 0u64), (1, 1), (255, OFFSET_MASK), (42, 1_234_567)] {
            assert_eq!(unpack(pack(tag, off)), (tag, off));
        }
    }

    #[test]
    fn uvarint_roundtrips() {
        let cases = [0u64, 1, 127, 128, 300, 16_383, 16_384, 1u64 << 40, u64::MAX];
        let mut buf = Vec::new();
        for &v in &cases {
            buf.clear();
            write_uvarint(&mut buf, v);
            let mut pos = 0;
            assert_eq!(read_uvarint(&buf, &mut pos), Some(v));
            assert_eq!(pos, buf.len(), "consumed exactly the bytes written for {v}");
        }
    }

    #[test]
    fn read_uvarint_handles_truncation() {
        // A continuation bit set with no following byte must not panic.
        let mut pos = 0;
        assert_eq!(read_uvarint(&[0x80], &mut pos), None);
    }

    #[test]
    fn container_roundtrips_sections() {
        let a = b"hello".to_vec();
        let b = b"world!!".to_vec();
        let mut buf = Vec::new();
        write_container(
            &mut buf,
            &[
                Section {
                    name: SEC_DESCRIPTIONS,
                    bytes: &a,
                },
                Section {
                    name: SEC_POSTINGS,
                    bytes: &b,
                },
            ],
        )
        .unwrap();

        let toc = Toc::parse(&buf).unwrap();
        let ra = toc.require(SEC_DESCRIPTIONS).unwrap();
        let rb = toc.require(SEC_POSTINGS).unwrap();
        assert_eq!(&buf[ra], &a[..]);
        assert_eq!(&buf[rb], &b[..]);
        assert!(toc.require("nonexistent").is_err());
    }

    #[test]
    fn rejects_bad_magic() {
        let mut buf = vec![0u8; 64];
        buf[..8].copy_from_slice(b"NOTANFST");
        assert!(Toc::parse(&buf).is_err());
    }
}
