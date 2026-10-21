//! Request/response layer over the 64-byte HID reports.
//!
//! Framing (from the Windows app): `payload[0]` = command, `payload[1]` =
//! total length including cmd+len for writes (0 or a sub-index for reads),
//! data after that, zero padded. No checksum.
//!
//! The original app routes a reply by the command it is *expecting*, not by
//! `rx[0]`; the only packets recognised by their own first byte are the
//! unsolicited 0x89 (device debug log), 0x8a ("Send Message Success") and
//! 0x8e ("Sync Time Success"). We mirror that: the next packet that is not a
//! 0x89 log line is taken as the reply.

use crate::hid::{Report, Transport, REPORT_LEN};
use anyhow::{bail, Result};
use std::time::{Duration, Instant};

pub const LOG_LINE: u8 = 0x89;

/// Default reply timeout used by the original app (0x7d0 ms).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2000);

pub struct Client<T: Transport> {
    t: T,
    pub debug: bool,
    /// Text lines received via unsolicited 0x89 packets.
    pub device_log: Vec<String>,
}

pub fn packet(bytes: &[u8]) -> Report {
    let mut r = [0u8; REPORT_LEN];
    let n = bytes.len().min(REPORT_LEN);
    r[..n].copy_from_slice(&bytes[..n]);
    r
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

/// Trim trailing zero padding for display.
fn trimmed(r: &Report) -> &[u8] {
    let end = r.iter().rposition(|&b| b != 0).map_or(1, |i| i + 1);
    &r[..end]
}

impl<T: Transport> Client<T> {
    pub fn new(t: T) -> Self {
        Self {
            t,
            debug: false,
            device_log: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn transport(&self) -> &T {
        &self.t
    }

    /// Swap in a freshly opened device (after a USB re-enumeration).
    pub fn replace_transport(&mut self, t: T) {
        self.t = t;
    }

    pub fn send(&mut self, pkt: &Report) -> Result<()> {
        if self.debug {
            eprintln!(">> {}", hex(trimmed(pkt)));
        }
        self.t.write_report(pkt)?;
        // The Windows app sleeps 5 ms after every write; the bootloader relies
        // on that pacing because program packets are not acknowledged.
        std::thread::sleep(Duration::from_millis(5));
        Ok(())
    }

    /// Receive the next non-log packet, or `None` on timeout.
    pub fn recv(&mut self, timeout: Duration) -> Result<Option<Report>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(r) = self.t.read_report(left)? else {
                return Ok(None);
            };
            if self.debug {
                eprintln!("<< {}", hex(trimmed(&r)));
            }
            if r[0] == LOG_LINE {
                let n = (r[2] as usize).min(REPORT_LEN - 3);
                let line = String::from_utf8_lossy(&r[3..3 + n]).into_owned();
                if self.debug {
                    eprintln!("   [device log] {}", line.trim_end());
                }
                self.device_log.push(line);
                continue;
            }
            return Ok(Some(r));
        }
    }

    /// Device log text received so far (the firmware forwards its modem
    /// AT traffic and status messages as unsolicited 0x89 packets).
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.device_log)
    }

    /// Drop anything already queued (stale replies from an earlier request).
    pub fn drain(&mut self) -> Result<()> {
        while self.recv(Duration::from_millis(0))?.is_some() {}
        Ok(())
    }

    /// Send a request and wait for one reply packet.
    pub fn request(&mut self, pkt: &Report, expect: u8, timeout: Duration) -> Result<Report> {
        self.drain()?;
        self.send(pkt)?;
        match self.recv(timeout)? {
            Some(r) => {
                if r[0] != expect && self.debug {
                    eprintln!(
                        "   note: reply cmd 0x{:02x}, expected 0x{expect:02x}",
                        r[0]
                    );
                }
                Ok(r)
            }
            None => bail!(
                "timeout waiting for reply 0x{expect:02x} to command 0x{:02x}",
                pkt[0]
            ),
        }
    }

    /// Simple read: `[cmd, 0...]`.
    pub fn read(&mut self, cmd: u8) -> Result<Report> {
        self.request(&packet(&[cmd]), cmd, DEFAULT_TIMEOUT)
    }

    /// Write whose acknowledgement content is ignored (as in the original
    /// app). A missing ack is reported as a warning, not an error, because
    /// the original never checked it either.
    pub fn write(&mut self, pkt: &Report, expect: u8) -> Result<()> {
        if let Err(e) = self.request(pkt, expect, DEFAULT_TIMEOUT) {
            eprintln!("warning: {e} (the write may still have been applied)");
        }
        Ok(())
    }

    /// Fire and forget (bootloader commands with no reply).
    pub fn send_only(&mut self, pkt: &Report) -> Result<()> {
        self.send(pkt)
    }

    /// Read a reply split over two packets, each carrying `p[2..p[1])`
    /// (used by 0x0f max/min).
    pub fn read_two_part(&mut self, cmd: u8) -> Result<Vec<u8>> {
        let first = self.read(cmd)?;
        let mut data = payload_data(&first).to_vec();
        match self.recv(DEFAULT_TIMEOUT)? {
            Some(second) => data.extend_from_slice(payload_data(&second)),
            None => bail!("timeout waiting for second part of 0x{cmd:02x} reply"),
        }
        Ok(data)
    }

    /// 0x8f memory read: `8f 06 sub addrlo addrhi count`; reply data is
    /// `p[2..p[1])`. Repeats until `total` bytes have been collected.
    pub fn read_memory(&mut self, sub: u8, start: u16, total: usize, chunk: u8) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(total);
        while out.len() < total {
            let addr = start as usize + out.len();
            // The original always asks for the full chunk, even at the end.
            let pkt = packet(&[0x8f, 0x06, sub, addr as u8, (addr >> 8) as u8, chunk]);
            let r = self.request(&pkt, 0x8f, DEFAULT_TIMEOUT)?;
            let d = payload_data(&r);
            if d.is_empty() {
                bail!("device returned no data for memory read at 0x{addr:x}");
            }
            out.extend_from_slice(d);
        }
        out.truncate(total);
        Ok(out)
    }
}

/// `p[2..p[1])`, clamped to the report.
pub fn payload_data(r: &Report) -> &[u8] {
    let end = (r[1] as usize).clamp(2, REPORT_LEN);
    &r[2..end]
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::VecDeque;

    /// Scripted transport: records writes; each write releases the next
    /// batch of replies, like a device answering one request at a time.
    #[derive(Default)]
    pub struct Mock {
        pub written: Vec<Report>,
        pub script: VecDeque<Vec<Report>>,
        pending: VecDeque<Report>,
    }

    impl Mock {
        /// Queue the replies the device sends after the next write.
        pub fn on_write(&mut self, replies: Vec<Report>) {
            self.script.push_back(replies);
        }
    }

    impl Transport for Mock {
        fn write_report(&mut self, r: &Report) -> Result<()> {
            self.written.push(*r);
            if let Some(batch) = self.script.pop_front() {
                self.pending.extend(batch);
            }
            Ok(())
        }
        fn read_report(&mut self, _t: Duration) -> Result<Option<Report>> {
            Ok(self.pending.pop_front())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::Mock;
    use super::*;

    #[test]
    fn skips_log_lines_and_collects_them() {
        let mut m = Mock::default();
        let mut log = [0u8; 64];
        log[0] = 0x89;
        log[2] = 5;
        log[3..8].copy_from_slice(b"hello");
        m.on_write(vec![log, packet(&[0x05, 0x12, 1])]);
        let mut c = Client::new(m);
        let r = c.request(&packet(&[0x05]), 0x05, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(r[0], 0x05);
        assert_eq!(c.device_log, vec!["hello".to_string()]);
    }

    #[test]
    fn memory_read_advances_address() {
        let mut m = Mock::default();
        let mut a = vec![0x8f, 62];
        a.extend(0..60u8);
        let mut b = vec![0x8f, 6];
        b.extend([60, 61, 62, 63]);
        m.on_write(vec![packet(&a)]);
        m.on_write(vec![packet(&b)]);
        let mut c = Client::new(m);
        let d = c.read_memory(1, 0, 64, 0x3c).unwrap();
        assert_eq!(d, (0..64u8).collect::<Vec<_>>());
        assert_eq!(&c.t.written[0][..6], &[0x8f, 6, 1, 0, 0, 0x3c]);
        assert_eq!(&c.t.written[1][..6], &[0x8f, 6, 1, 60, 0, 0x3c]);
    }
}
