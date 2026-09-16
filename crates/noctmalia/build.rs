//! Zips `bridge/` — the Thunderbird extension — into an XPI the binary embeds and writes into its
//! profile on every start. A stored (uncompressed) zip is a page of format and needs no crate;
//! Thunderbird reads it like any other, and entries in sorted order make the bytes reproducible,
//! which is what lets the launcher rewrite the file only when the extension actually changed.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let bridge = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bridge");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("bridge.xpi");
    let mut files = Vec::new();
    walk(&bridge, &bridge, &mut files);
    files.sort();
    for file in &files {
        println!("cargo:rerun-if-changed={}", bridge.join(file).display());
    }
    println!("cargo:rerun-if-changed={}", bridge.display());
    fs::write(&out, stored_zip(&bridge, &files)).expect("write bridge.xpi");
}

fn walk(root: &Path, dir: &Path, into: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("read bridge/") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            walk(root, &path, into);
        } else {
            let relative = path.strip_prefix(root).expect("under root");
            into.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn stored_zip(root: &Path, names: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for name in names {
        let bytes = fs::read(root.join(name)).expect("read entry");
        let crc = crc32(&bytes);
        let offset = out.len() as u32;
        // Local file header. A fixed DOS date (1980-01-01) keeps the archive reproducible.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        header_body(&mut out, crc, bytes.len() as u32, name);
        out.extend_from_slice(&bytes);
        // Central directory record for the same entry.
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        header_body(&mut central, crc, bytes.len() as u32, name);
        // The `header_body` wrote the name; the central record wants the file-comment length,
        // disk number, attributes and offset *before* it, so fix the order up.
        let name_len = name.len();
        let name_bytes = central.split_off(central.len() - name_len);
        central.extend_from_slice(&0u16.to_le_bytes()); // comment length
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(&name_bytes);
    }
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(names.len() as u16).to_le_bytes());
    out.extend_from_slice(&(names.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// The fields a local header and a central record share, in order: version needed, flags,
/// method (stored), time, date, crc, compressed size, size, name length, extra length, name.
fn header_body(out: &mut Vec<u8>, crc: u32, size: u32, name: &str) {
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0x0021u16.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.write_all(name.as_bytes()).expect("vec write");
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}
