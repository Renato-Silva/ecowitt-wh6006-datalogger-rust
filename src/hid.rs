//! Minimal Linux hidraw transport (no libusb / hidapi dependency).
//!
//! The logger enumerates as a Microchip HID device (VID 0x04D8, PID 0x003C)
//! using unnumbered 64-byte reports. On hidraw an unnumbered output report is
//! written as `[0x00, payload[0..64]]` and input reports are read as 64 bytes.

use anyhow::{bail, Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const VID: u16 = 0x04D8;
pub const PID: u16 = 0x003C;
pub const REPORT_LEN: usize = 64;

pub type Report = [u8; REPORT_LEN];

/// Anything that can exchange 64-byte reports. Lets the protocol layer be
/// tested without hardware.
pub trait Transport {
    fn write_report(&mut self, report: &Report) -> Result<()>;
    /// Returns `Ok(None)` on timeout.
    fn read_report(&mut self, timeout: Duration) -> Result<Option<Report>>;
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub path: PathBuf,
    pub vid: u16,
    pub pid: u16,
    pub name: String,
}

/// Scan `/sys/class/hidraw` for HID devices, optionally filtered by VID/PID.
pub fn enumerate(filter: Option<(u16, u16)>) -> Result<Vec<DeviceInfo>> {
    let mut out = Vec::new();
    let dir = match fs::read_dir("/sys/class/hidraw") {
        Ok(d) => d,
        Err(_) => return Ok(out),
    };
    for entry in dir.flatten() {
        let uevent = entry.path().join("device/uevent");
        let Ok(text) = fs::read_to_string(&uevent) else { continue };
        let mut ids = None;
        let mut name = String::new();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("HID_ID=") {
                ids = parse_hid_id(v);
            } else if let Some(v) = line.strip_prefix("HID_NAME=") {
                name = v.to_string();
            }
        }
        let Some((vid, pid)) = ids else { continue };
        if filter.is_some_and(|f| f != (vid, pid)) {
            continue;
        }
        out.push(DeviceInfo {
            path: Path::new("/dev").join(entry.file_name()),
            vid,
            pid,
            name,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// `HID_ID=0003:000004D8:0000003C` -> (0x04D8, 0x003C)
fn parse_hid_id(v: &str) -> Option<(u16, u16)> {
    let mut it = v.split(':');
    let _bus = it.next()?;
    let vid = u32::from_str_radix(it.next()?, 16).ok()?;
    let pid = u32::from_str_radix(it.next()?, 16).ok()?;
    Some((vid as u16, pid as u16))
}

pub struct HidDevice {
    file: File,
}

impl HidDevice {
    pub fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| {
                format!(
                    "opening {} (do you need a udev rule? see README)",
                    path.display()
                )
            })?;
        Ok(Self { file })
    }

    /// Open the first logger found, or the explicit path if given.
    pub fn open_logger(path: Option<&Path>) -> Result<Self> {
        if let Some(p) = path {
            return Self::open(p);
        }
        let devs = enumerate(Some((VID, PID)))?;
        let Some(dev) = devs.first() else {
            bail!("no Weather Logger (USB {VID:04x}:{PID:04x}) found");
        };
        Self::open(&dev.path)
    }
}

impl Transport for HidDevice {
    fn write_report(&mut self, report: &Report) -> Result<()> {
        let mut buf = [0u8; REPORT_LEN + 1];
        buf[1..].copy_from_slice(report);
        let n = self.file.write(&buf).context("hidraw write")?;
        if n != buf.len() {
            bail!("short hidraw write ({n} of {})", buf.len());
        }
        Ok(())
    }

    fn read_report(&mut self, timeout: Duration) -> Result<Option<Report>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let mut pfd = libc::pollfd {
                fd: self.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ms = left.as_millis().min(i32::MAX as u128) as i32;
            // SAFETY: pfd is a valid pollfd for the duration of the call.
            let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err).context("poll on hidraw");
            }
            if rc == 0 {
                return Ok(None);
            }
            let mut buf = [0u8; REPORT_LEN + 1];
            let n = self.file.read(&mut buf).context("hidraw read")?;
            if n == 0 {
                bail!("device disconnected");
            }
            let mut report = [0u8; REPORT_LEN];
            let n = n.min(REPORT_LEN);
            report[..n].copy_from_slice(&buf[..n]);
            return Ok(Some(report));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hid_id() {
        assert_eq!(
            parse_hid_id("0003:000004D8:0000003C"),
            Some((0x04D8, 0x003C))
        );
        assert_eq!(parse_hid_id("garbage"), None);
    }
}
