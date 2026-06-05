//! Rock Ridge (IEEE P1282) over SUSP (IEEE P1281) — the POSIX layer on ISO9660.
//!
//! Each directory record carries a "System Use" area after its file identifier;
//! SUSP packs tagged entries there. Rock Ridge uses them to restore what plain
//! ISO9660 throws away: real (case-preserving, long, UTF-8) names (`NM`), POSIX
//! mode (`PX`), and symlinks (`SL`). This module parses the in-record System Use
//! area; `CE` continuation areas (used when the attributes overflow a record —
//! long names, many entries) are detected but deferred to a later pass.
//!
//! Clean-roomed from the public IEEE P1281/P1282 draft specifications.

use alloc::string::String;
use alloc::vec::Vec;

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

/// Parse Rock Ridge entries from a record's (skip-adjusted) System Use area.
pub fn parse_su(su: &[u8]) -> RrInfo {
    let mut info = RrInfo::default();
    let mut name = Vec::new();
    let mut have_nm = false;
    let mut sl = Vec::new();
    let mut have_sl = false;

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
                // data = [flags, name...]; flags bit1=CURRENT(.), bit2=PARENT(..)
                if !data.is_empty() && data[0] & 0x06 == 0 {
                    name.extend_from_slice(&data[1..]);
                    have_nm = true;
                }
            }
            b"PX" => {
                if data.len() >= 4 {
                    info.mode = Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]));
                }
            }
            b"SL" => {
                have_sl = true;
                if !data.is_empty() {
                    parse_sl_components(&data[1..], &mut sl);
                }
            }
            b"ST" => break,
            // CE (continuation), TF (timestamps), CL/PL/RE (deep-dir relocation):
            // not yet consumed — see module docs.
            _ => {}
        }
        p += len;
    }

    if have_nm {
        if let Ok(s) = String::from_utf8(name) {
            if !s.is_empty() {
                info.name = Some(s);
            }
        }
    }
    if have_sl {
        info.symlink_target = Some(sl);
    }
    info
}

/// Append the path components of an `SL` entry to `out`, separated by `/`.
fn parse_sl_components(mut comp: &[u8], out: &mut Vec<u8>) {
    // Each component: [flags, len, content[len]]. flags: bit1=CURRENT(.),
    // bit2=PARENT(..), bit3=ROOT(/).
    while comp.len() >= 2 {
        let flags = comp[0];
        let clen = comp[1] as usize;
        if 2 + clen > comp.len() {
            break;
        }
        let content = &comp[2..2 + clen];
        let is_root = flags & 0x08 != 0;
        if !out.is_empty() && !is_root {
            out.push(b'/');
        }
        if is_root {
            if out.is_empty() {
                out.push(b'/');
            }
        } else if flags & 0x02 != 0 {
            out.extend_from_slice(b".");
        } else if flags & 0x04 != 0 {
            out.extend_from_slice(b"..");
        } else {
            out.extend_from_slice(content);
        }
        comp = &comp[2 + clen..];
    }
}
