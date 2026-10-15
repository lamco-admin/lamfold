//! Rock Ridge (IEEE P1282) over SUSP (IEEE P1281) — the POSIX layer on ISO9660.
//!
//! Each directory record carries a "System Use" area after its file identifier;
//! SUSP packs tagged entries there. Rock Ridge uses them to restore what plain
//! ISO9660 throws away: real (case-preserving, long, UTF-8) names (`NM`), POSIX
//! mode (`PX`), and symlinks (`SL`). When the entries overflow a record (a long
//! name, a long link target) the rest sits in a continuation area that a `CE`
//! entry points to; `RrParser` reports it, the caller reads it and feeds it in.
//!
//! Clean-roomed from the public IEEE P1281/P1282 draft specifications.

use alloc::string::String;
use alloc::vec::Vec;

/// zisofs (transparent paged-zlib compression) parameters from the `ZF` entry.
#[derive(Clone, Copy)]
pub struct Zisofs {
    /// The file's real (decompressed) length.
    pub uncompressed_size: u32,
    /// log2 of the compression block size (e.g. 15 ⇒ 32 KiB blocks).
    pub block_size_log2: u8,
}

/// What Rock Ridge adds to a directory entry. All optional — absent fields mean
/// "fall back to the base ISO9660 value".
#[derive(Default)]
pub struct RrInfo {
    /// Real POSIX name from `NM` (overrides the 8.3 base name).
    pub name: Option<String>,
    /// POSIX `st_mode` from `PX`.
    pub mode: Option<u32>,
    /// Symlink target from `SL` (presence ⇒ the entry is a symlink).
    pub symlink_target: Option<Vec<u8>>,
    /// zisofs compression parameters from `ZF` (presence ⇒ the file is zisofs).
    pub zisofs: Option<Zisofs>,
}

/// Detect the SUSP `SP` indicator (only in the root directory's "." record) and
/// return its "skip" length — bytes to ignore at the start of every System Use
/// area. `None` ⇒ no SUSP/Rock Ridge on this volume.
pub fn detect_sp(su: &[u8]) -> Option<usize> {
    let mut p = 0;
    while p + 4 <= su.len() {
        let len = su[p + 2] as usize;
        if len < 4 || p + len > su.len() {
            break;
        }
        if &su[p..p + 2] == b"SP" && len >= 7 && su[p + 4] == 0xBE && su[p + 5] == 0xEF {
            return Some(su[p + 6] as usize);
        }
        p += len;
    }
    None
}

/// Where a record's entries continue: a `CE` entry's logical block, offset in
/// it, and length.
pub struct Continuation {
    pub block: u32,
    pub offset: u32,
    pub len: u32,
}

/// Rock Ridge entries collected across a record's System Use area and its
/// continuation areas.
#[derive(Default)]
pub struct RrParser {
    info: RrInfo,
    name: Vec<u8>,
    have_nm: bool,
    sl: Vec<u8>,
    have_sl: bool,
    /// The last `SL` component continues in the next one, without a `/`.
    sl_continues: bool,
}

impl RrParser {
    /// Parse one System Use area (the record's, skip-adjusted, or a
    /// continuation area) and return the continuation area it points to.
    pub fn feed(&mut self, su: &[u8]) -> Option<Continuation> {
        let mut next = None;
        let mut p = 0;
        while p + 4 <= su.len() {
            let len = su[p + 2] as usize;
            if len < 4 || p + len > su.len() {
                break;
            }
            let sig = &su[p..p + 2];
            let data = &su[p + 4..p + len];
            match sig {
                b"NM" => {
                    // data = [flags, name...]; flags bit0 = CONTINUE (the name
                    // goes on in the next NM), bit1 = CURRENT, bit2 = PARENT.
                    if !data.is_empty() && data[0] & 0x06 == 0 {
                        self.name.extend_from_slice(&data[1..]);
                        self.have_nm = true;
                    }
                }
                b"PX" => {
                    if data.len() >= 4 {
                        self.info.mode =
                            Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]));
                    }
                }
                b"SL" => {
                    self.have_sl = true;
                    if !data.is_empty() {
                        parse_sl_components(&data[1..], &mut self.sl, &mut self.sl_continues);
                    }
                }
                b"ZF" => {
                    // [algo(2)="pz", header_size/4, block_size_log2, uncompressed_size(4 LE)]
                    if data.len() >= 8 && &data[0..2] == b"pz" {
                        self.info.zisofs = Some(Zisofs {
                            block_size_log2: data[3],
                            uncompressed_size: u32::from_le_bytes([
                                data[4], data[5], data[6], data[7],
                            ]),
                        });
                    }
                }
                b"CE" => {
                    // Both-endian block, offset and length: the LE halves at 0, 8, 16.
                    if data.len() >= 24 {
                        let le = |o: usize| {
                            u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]])
                        };
                        next = Some(Continuation {
                            block: le(0),
                            offset: le(8),
                            len: le(16),
                        });
                    }
                }
                b"ST" => break,
                // TF (timestamps), CL/PL/RE (deep-dir relocation): not consumed.
                _ => {}
            }
            p += len;
        }
        next
    }

    pub fn finish(mut self) -> RrInfo {
        if self.have_nm {
            if let Ok(s) = String::from_utf8(self.name) {
                if !s.is_empty() {
                    self.info.name = Some(s);
                }
            }
        }
        if self.have_sl {
            self.info.symlink_target = Some(self.sl);
        }
        self.info
    }
}

/// Append the path components of an `SL` entry to `out`, separated by `/`.
/// A component with the CONTINUE flag goes on in the next component (of this
/// entry or the next `SL`) with no separator.
fn parse_sl_components(mut comp: &[u8], out: &mut Vec<u8>, continues: &mut bool) {
    // Each component: [flags, len, content[len]]. flags: bit0=CONTINUE,
    // bit1=CURRENT(.), bit2=PARENT(..), bit3=ROOT(/).
    while comp.len() >= 2 {
        let flags = comp[0];
        let clen = comp[1] as usize;
        if 2 + clen > comp.len() {
            break;
        }
        let content = &comp[2..2 + clen];
        if flags & 0x08 != 0 {
            // ROOT starts an absolute path; its "/" is also the separator.
            if out.is_empty() {
                out.push(b'/');
            }
        } else {
            if !*continues && !out.is_empty() && out.last() != Some(&b'/') {
                out.push(b'/');
            }
            if flags & 0x02 != 0 {
                out.extend_from_slice(b".");
            } else if flags & 0x04 != 0 {
                out.extend_from_slice(b"..");
            } else {
                out.extend_from_slice(content);
            }
        }
        *continues = flags & 0x01 != 0;
        comp = &comp[2 + clen..];
    }
}
