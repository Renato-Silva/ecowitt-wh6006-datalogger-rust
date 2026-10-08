//! Firmware update over the HID bootloader (Microchip-style, PIC24 image).
//!
//! Sequence reproduced from the Windows app:
//!   86 (enter bootloader) → sleep 1.5 s → 80 must report "Bootloader…"
//!   → 82 erase (reply[2] == 0) → 83 program chunks (no ACK, 5 ms pacing)
//!   → 84 (program complete) → sleep 8 s → 85 read-back verify
//!   → sleep 1 s → 87 (run application)

use crate::device::Logger;
use crate::hid::Transport;
use crate::protocol::packet;
use anyhow::{bail, ensure, Context, Result};
use std::time::Duration;

/// Bin offsets programmed by the app: [0x3000, 0x2A7F8). Below 0x3000 is
/// the bootloader itself and is never touched.
pub const PROG_START: usize = 0x3000;
pub const PROG_END: usize = 0x2A7F8;
pub const CHUNK: usize = 0x38;

/// Image in "bin" layout: byte offset == Intel HEX byte address
/// (== 2 × PIC24 program counter), with every 4th (phantom) byte zero.
pub struct Image {
    pub data: Vec<u8>,
    /// Data bytes found in the HEX outside the programmed window.
    pub outside: usize,
    pub inside: usize,
}

pub fn parse_hex(text: &str) -> Result<Image> {
    let mut data = vec![0xFFu8; PROG_END];
    let mut base: u32 = 0;
    let mut inside = 0;
    let mut outside = 0;
    let mut saw_eof = false;
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let ln = lineno + 1;
        let rec = line
            .strip_prefix(':')
            .with_context(|| format!("line {ln}: record does not start with ':'"))?;
        ensure!(rec.len() % 2 == 0 && rec.len() >= 10, "line {ln}: bad record length");
        let bytes = (0..rec.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&rec[i..i + 2], 16))
            .collect::<std::result::Result<Vec<u8>, _>>()
            .with_context(|| format!("line {ln}: invalid hex digit"))?;
        let n = bytes[0] as usize;
        ensure!(bytes.len() == n + 5, "line {ln}: length field mismatch");
        let sum = bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b));
        ensure!(sum == 0, "line {ln}: checksum error");
        let off = u16::from_be_bytes([bytes[1], bytes[2]]) as u32;
        let payload = &bytes[4..4 + n];
        match bytes[3] {
            0x00 => {
                for (i, b) in payload.iter().enumerate() {
                    let addr = (base + off + i as u32) as usize;
                    if (PROG_START..PROG_END).contains(&addr) {
                        data[addr] = *b;
                        inside += 1;
                    } else {
                        if addr < PROG_END {
                            data[addr] = *b;
                        }
                        outside += 1;
                    }
                }
            }
            0x01 => {
                saw_eof = true;
                break;
            }
            0x02 => base = (u16::from_be_bytes([payload[0], payload[1]]) as u32) << 4,
            0x04 => base = (u16::from_be_bytes([payload[0], payload[1]]) as u32) << 16,
            // Start-address records: no effect on the image. (The original
            // app wrongly let these overwrite the extended base address.)
            0x03 | 0x05 => {}
            t => bail!("line {ln}: unsupported record type {t:02x}"),
        }
    }
    ensure!(saw_eof, "HEX file has no end-of-file record");
    // PIC24 phantom byte: every 4th byte is zero (the app forces this).
    for i in (3..data.len()).step_by(4) {
        data[i] = 0;
    }
    Ok(Image { data, outside, inside })
}

fn program_packet(cmd: u8, off: usize, chunk: &[u8]) -> crate::hid::Report {
    let pc = (off >> 1) as u32;
    let mut b = vec![cmd];
    b.extend_from_slice(&pc.to_le_bytes());
    b.push(chunk.len() as u8);
    b.extend_from_slice(&[0, 0]);
    b.extend_from_slice(chunk);
    packet(&b)
}

/// Chunk offsets used for both programming and verify, including the
/// trailing zero-length chunk the app sends.
fn chunks() -> impl Iterator<Item = (usize, usize)> {
    let len = PROG_END - PROG_START;
    (0..=len / CHUNK).map(move |k| {
        let off = PROG_START + k * CHUNK;
        (off, (PROG_END - off).min(CHUNK))
    })
}

pub fn update<T: Transport>(
    lg: &mut Logger<T>,
    img: &Image,
    reopen: &mut dyn FnMut() -> Result<T>,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<()> {
    for attempt in 0..2 {
        match update_once(lg, img, reopen, progress)? {
            true => return Ok(()),
            false if attempt == 0 => {
                eprintln!("Verification failed; retrying the whole update once (as the original app does)");
            }
            false => {}
        }
    }
    bail!("Verification failed! Please update the firmware again.")
}

/// Ask for the version, reopening the device if it re-enumerated.
fn query_version<T: Transport>(
    lg: &mut Logger<T>,
    reopen: &mut dyn FnMut() -> Result<T>,
) -> Result<crate::model::Version> {
    for _ in 0..6 {
        match lg.version() {
            Ok(v) => return Ok(v),
            Err(_) => {
                std::thread::sleep(Duration::from_millis(1000));
                if let Ok(t) = reopen() {
                    lg.c.replace_transport(t);
                }
            }
        }
    }
    bail!("CMD_SOFT_VER Timeout")
}

fn update_once<T: Transport>(
    lg: &mut Logger<T>,
    img: &Image,
    reopen: &mut dyn FnMut() -> Result<T>,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<bool> {
    // 1. enter bootloader
    lg.c.send_only(&packet(&[0x86]))?;
    std::thread::sleep(Duration::from_millis(1500));

    // 2. confirm bootloader mode
    let v = query_version(lg, reopen)?;
    if !v.is_bootloader() {
        bail!("Error1! device did not enter bootloader mode (reports {:?})", v.label);
    }
    eprintln!("{} : {}", v.label, v.version);

    // 3. erase
    let mut erased = false;
    for _ in 0..6 {
        match lg.c.request(&packet(&[0x82]), 0x82, Duration::from_secs(10)) {
            Ok(r) if r[2] == 0 => {
                erased = true;
                break;
            }
            Ok(r) => bail!("Error2! erase failed (status 0x{:02x})", r[2]),
            Err(_) => continue,
        }
    }
    ensure!(erased, "CMD_ERASE_DEVICE Timeout");

    // 4. program (no acknowledgements; pacing is the 5 ms post-write sleep)
    let all: Vec<_> = chunks().collect();
    for (i, (off, n)) in all.iter().enumerate() {
        lg.c.send_only(&program_packet(0x83, *off, &img.data[*off..*off + *n]))?;
        progress("program", i + 1, all.len());
    }
    lg.c.send_only(&packet(&[0x84]))?;
    std::thread::sleep(Duration::from_secs(8));

    // 5. verify, matching replies by order like the app
    let mut timeouts = 0;
    for (i, (off, n)) in all.iter().enumerate() {
        if *n == 0 {
            continue;
        }
        let req = program_packet(0x85, *off, &[]);
        let mut r = lg.c.request(&req, 0x85, Duration::from_secs(1));
        if r.is_err() {
            timeouts += 1;
            if timeouts >= 2 {
                eprintln!("verify: too many timeouts");
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(100));
            r = lg.c.request(&req, 0x85, Duration::from_secs(1));
        }
        let Ok(r) = r else { return Ok(false) };
        let got = (r[5] as usize).min(56);
        if got != *n || r[8..8 + got] != img.data[*off..*off + got] {
            eprintln!("verify: mismatch at offset 0x{off:x}");
            return Ok(false);
        }
        progress("verify", i + 1, all.len());
    }

    // 6. run application
    std::thread::sleep(Duration::from_secs(1));
    lg.c.send_only(&packet(&[0x87]))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(addr: u16, ty: u8, data: &[u8]) -> String {
        let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, ty];
        b.extend_from_slice(data);
        let sum = b.iter().fold(0u8, |a, x| a.wrapping_add(*x));
        b.push(sum.wrapping_neg());
        format!(":{}", b.iter().map(|x| format!("{x:02X}")).collect::<String>())
    }

    #[test]
    fn hex_parsing_and_phantom_bytes() {
        let text = [
            rec(0, 0x04, &[0x00, 0x00]),
            rec(0x3000, 0x00, &[1, 2, 3, 4, 5, 6, 7, 8]),
            rec(0, 0x04, &[0x00, 0x01]),
            rec(0x0000, 0x00, &[9, 9]),
            rec(0, 0x01, &[]),
        ]
        .join("\n");
        let img = parse_hex(&text).unwrap();
        assert_eq!(&img.data[0x3000..0x3008], &[1, 2, 3, 0, 5, 6, 7, 0]);
        assert_eq!(&img.data[0x10000..0x10002], &[9, 9]);
        assert_eq!(img.data[0x3009], 0xFF);
        assert_eq!(img.inside, 10);
    }

    #[test]
    fn rejects_bad_checksum() {
        assert!(parse_hex(":0100000001FF\n:00000001FF").is_err());
    }

    #[test]
    fn chunking_matches_app() {
        let c: Vec<_> = chunks().collect();
        assert_eq!(c.len(), 2890);
        assert_eq!(c[0], (0x3000, 56));
        assert_eq!(c.last().unwrap().1, 0);
        let p = program_packet(0x83, 0x3000, &[0xAA; 56]);
        assert_eq!(&p[..8], &[0x83, 0x00, 0x18, 0, 0, 56, 0, 0]);
    }
}
