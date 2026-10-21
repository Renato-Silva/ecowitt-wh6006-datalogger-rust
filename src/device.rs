//! High-level operations built on the request/response client.

use crate::hid::{Report, Transport};
use crate::model::*;
use crate::protocol::{packet, Client, DEFAULT_TIMEOUT};
use anyhow::{bail, ensure, Context, Result};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

pub struct Logger<T: Transport> {
    pub c: Client<T>,
}

impl<T: Transport> Logger<T> {
    pub fn new(t: T, debug: bool) -> Self {
        let mut c = Client::new(t);
        c.debug = debug;
        Self { c }
    }

    pub fn version(&mut self) -> Result<Version> {
        let r = self
            .c
            .request(&packet(&[0x80]), 0x80, Duration::from_millis(1000))?;
        Version::decode(&r)
    }

    /// The app only talks to the application firmware; if the device sits
    /// in its bootloader the app sends 0x87 (run application).
    pub fn ensure_app_mode(&mut self) -> Result<Version> {
        let mut v = self.version()?;
        for _ in 0..5 {
            if !v.is_bootloader() {
                return Ok(v);
            }
            eprintln!("device is in bootloader mode, asking it to start the application...");
            self.c.send_only(&packet(&[0x87]))?;
            std::thread::sleep(Duration::from_millis(1500));
            v = self.version()?;
        }
        if v.is_bootloader() {
            bail!("device stays in bootloader mode (no valid firmware?); use `firmware update`");
        }
        Ok(v)
    }

    pub fn name(&mut self) -> Result<String> {
        let r = self.c.request(&packet(&[0x19]), 0x19, Duration::from_millis(300))?;
        let n = r[1] as usize;
        if n == 0 || n >= 0x14 {
            return Ok(String::new());
        }
        Ok(String::from_utf8_lossy(&r[2..n]).into_owned())
    }

    pub fn live(&mut self) -> Result<Live> {
        let b = self.c.read_memory(1, 0, LIVE_LEN, 0x3c)?;
        Live::decode(&b)
    }

    pub fn sensor(&mut self, slot: u8) -> Result<SensorRecord> {
        let b = self.c.read_memory(2, slot as u16 * 20, 20, 20)?;
        SensorRecord::decode(slot, &b)
    }

    pub fn set_sensor_id(&mut self, slot: u8, id: u32) -> Result<()> {
        self.c.write(&encode_sensor_id(slot, id), 0x91)
    }

    pub fn status(&mut self) -> Result<Status> {
        Ok(Status::decode(&self.c.read(0x18)?))
    }

    pub fn maxmin(&mut self) -> Result<Vec<MaxMinItem>> {
        Ok(decode_maxmin(&self.c.read_two_part(0x0f)?))
    }

    pub fn clear_maxmin(&mut self) -> Result<()> {
        self.c.write(&packet(CLEAR_MAXMIN), 0x10)
    }

    pub fn settings(&mut self) -> Result<Settings> {
        Settings::decode(&self.c.read(0x05)?)
    }

    pub fn set_settings(&mut self, s: &Settings) -> Result<()> {
        self.c.write(&s.encode()?, 0x06)
    }

    pub fn sync_time_utc(&mut self) -> Result<()> {
        use chrono::{Datelike, Timelike, Utc};
        let n = Utc::now();
        let p = encode_time(
            n.year() as u16,
            n.month() as u8,
            n.day() as u8,
            n.hour() as u8,
            n.minute() as u8,
            n.second() as u8,
        );
        self.c.write(&p, 0x02)
    }

    pub fn sync_net_time(&mut self) -> Result<()> {
        self.c.write(&packet(&[0x8e]), 0x8e)
    }

    pub fn set_rain(&mut self, p: Report) -> Result<()> {
        self.c.write(&p, 0x02)
    }

    pub fn calibration(&mut self) -> Result<Calibration> {
        Calibration::decode(&self.c.read(0x03)?)
    }

    pub fn set_calibration(&mut self, cal: &Calibration) -> Result<()> {
        self.c.write(&cal.encode()?, 0x04)
    }

    pub fn alarms(&mut self) -> Result<Alarms> {
        Alarms::decode(&self.c.read(0x0d)?)
    }

    pub fn set_alarms(&mut self, a: &Alarms) -> Result<()> {
        self.c.write(&a.encode()?, 0x0e)
    }

    pub fn mobile(&mut self) -> Result<Mobile> {
        let apn = self.c.read(0x11)?;
        let cred = self.c.read(0x13)?;
        let sms = self.c.read(0x07)?;
        Mobile::decode(&apn, &cred, &sms)
    }

    pub fn set_mobile(&mut self, m: &Mobile) -> Result<()> {
        let [a, c, s] = m.encode()?;
        self.c.write(&a, 0x12)?;
        self.c.write(&c, 0x14)?;
        self.c.write(&s, 0x08)
    }

    /// The app saves the mobile settings before sending the test SMS; we
    /// leave that to the caller.
    pub fn test_sms(&mut self) -> Result<()> {
        self.c.write(&encode_test_sms(), 0x8a)
    }

    pub fn upload_server(&mut self, sub: u8) -> Result<UploadServer> {
        ensure!(sub < 4, "server index must be 0..3");
        let h = self.c.request(&packet(&[0x09, sub]), 0x09, DEFAULT_TIMEOUT)?;
        let p = self.c.request(&packet(&[0x0b, sub]), 0x0b, DEFAULT_TIMEOUT)?;
        UploadServer::decode(&h, &p)
    }

    /// All four servers, read in the app's order: `09 0..3`, then `0b 0..3`.
    pub fn upload_servers(&mut self) -> Result<Vec<UploadServer>> {
        let mut hosts = Vec::new();
        for sub in 0..4u8 {
            hosts.push(self.c.request(&packet(&[0x09, sub]), 0x09, DEFAULT_TIMEOUT)?);
        }
        let mut all = Vec::new();
        for (sub, h) in (0..4u8).zip(&hosts) {
            let p = self.c.request(&packet(&[0x0b, sub]), 0x0b, DEFAULT_TIMEOUT)?;
            all.push(UploadServer::decode(h, &p)?);
        }
        Ok(all)
    }

    /// The app always saves the whole table: `0a 0c` for sub 0, 1, 2, 3, each
    /// acknowledged with 0x88. Writing a single server is not something the
    /// firmware was ever sent, so we don't either.
    pub fn set_upload_servers(&mut self, all: &[UploadServer]) -> Result<()> {
        ensure!(
            all.len() == 4 && all.iter().enumerate().all(|(i, u)| u.sub as usize == i),
            "need the four upload servers in order"
        );
        for u in all {
            let [h, p] = u.encode()?;
            self.c.write(&h, 0x88)?;
            self.c.write(&p, 0x88)?;
        }
        Ok(())
    }

    pub fn reset_device(&mut self) -> Result<()> {
        // 0x86 reboots into the bootloader; the app's heartbeat then sends
        // 0x87 to start the application again. Do the same.
        self.c.send_only(&packet(&[0x86]))?;
        std::thread::sleep(Duration::from_millis(1500));
        for _ in 0..6 {
            match self.version() {
                Ok(v) if v.is_bootloader() => {
                    self.c.send_only(&packet(&[0x87]))?;
                    std::thread::sleep(Duration::from_millis(1000));
                }
                Ok(_) => return Ok(()),
                Err(_) => std::thread::sleep(Duration::from_millis(1000)),
            }
        }
        eprintln!("warning: could not confirm the device came back up; replug it if needed");
        Ok(())
    }

    pub fn factory_reset(&mut self) -> Result<()> {
        self.c.write(&packet(&[0x88]), 0x88)
    }

    // ----------------------------------------------------------------- SD

    pub fn sd_list(&mut self) -> Result<(Vec<SdFile>, Option<u8>)> {
        let mut files = Vec::new();
        self.c.drain()?;
        self.c.send(&packet(&[0x15]))?;
        loop {
            let Some(r) = self.c.recv(DEFAULT_TIMEOUT)? else {
                bail!("timeout reading SD card file list");
            };
            ensure!(r[0] == 0x15, "Read SDCard fail (got packet 0x{:02x})", r[0]);
            match decode_sd_entry(&r) {
                Some(f) => files.push(f),
                None => break,
            }
            ensure!(files.len() <= 500, "too many files in SD listing");
        }
        // The app follows the listing with 0x18 for the SD card status.
        let sd = self.status().ok().map(|s| s.sd_status);
        Ok((files, sd))
    }

    /// Download (or resume) `name` into `dest`. Returns bytes received.
    ///
    /// Resuming follows the device's byte count: the local file is kept
    /// byte-identical to the device file (LF line endings), unlike the
    /// Windows app which re-encodes as CRLF.
    pub fn sd_download(
        &mut self,
        file: &SdFile,
        dest: &Path,
        resume: bool,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<u64> {
        let local_len = if resume {
            std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        let mut offset = local_len as u32;
        if offset > file.size {
            eprintln!("local file is larger than the device copy; downloading from scratch");
            offset = 0;
        }
        let remaining = file.size - offset;
        if remaining == 0 {
            return Ok(0);
        }
        let mut out = OpenOptions::new()
            .create(true)
            .write(true)
            .append(offset > 0)
            .truncate(offset == 0)
            .open(dest)
            .with_context(|| format!("creating {}", dest.display()))?;

        self.c.drain()?;
        self.c.send(&encode_sd_download(&file.name, offset, remaining)?)?;

        let mut got: u64 = 0;
        // The app's watchdog gives up after ~5 s without a new chunk.
        let idle = Duration::from_secs(5);
        let mut last = Instant::now();
        loop {
            let Some(r) = self.c.recv(Duration::from_millis(200))? else {
                if last.elapsed() > idle {
                    bail!("Read file errors: no data for 5 s after {got} bytes (partial file kept; rerun with --resume)");
                }
                continue;
            };
            last = Instant::now();
            if r[0] != 0x16 {
                continue;
            }
            let l = (r[2] as usize).min(61);
            out.write_all(&r[3..3 + l])?;
            got += l as u64;
            progress(got, remaining as u64);
            if l < SD_FULL_CHUNK || got >= remaining as u64 {
                break;
            }
        }
        out.flush()?;
        Ok(got)
    }

    pub fn sd_delete(&mut self, name: &str) -> Result<()> {
        self.c.drain()?;
        self.c.send(&encode_sd_delete(name)?)?;
        // The app just waits 500 ms and doesn't inspect the reply.
        let _ = self.c.recv(Duration::from_millis(500))?;
        Ok(())
    }

    // ---------------------------------------------------------- raw / debug

    pub fn raw(&mut self, bytes: &[u8], wait: Duration, count: usize) -> Result<Vec<Report>> {
        self.c.drain()?;
        self.c.send(&packet(bytes))?;
        let mut out = Vec::new();
        while out.len() < count {
            match self.c.recv(wait)? {
                Some(r) => out.push(r),
                None => break,
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::mock::Mock;

    fn chunk(data: &[u8]) -> Report {
        let mut v = vec![0x16, 0, data.len() as u8];
        v.extend_from_slice(data);
        packet(&v)
    }

    #[test]
    fn sd_download_stops_on_short_chunk() {
        let mut m = Mock::default();
        let a = [b'a'; 61];
        m.on_write(vec![chunk(&a), chunk(b"tail\n")]);
        let mut lg = Logger::new(m, false);
        let dir = std::env::temp_dir().join(format!("wh6006-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("t.csv");
        let f = SdFile { name: "t.csv".into(), size: 66 };
        let n = lg.sd_download(&f, &dest, false, |_, _| {}).unwrap();
        assert_eq!(n, 66);
        let body = std::fs::read(&dest).unwrap();
        assert_eq!(body.len(), 66);
        assert!(body.ends_with(b"tail\n"));
        let req = lg.c_written(0);
        assert_eq!(&req[..2], &[0x16, 5 + 11]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sd_list_reads_until_end_marker() {
        let mut m = Mock::default();
        let mut e = vec![0x15, 0];
        e.extend_from_slice(&100u32.to_le_bytes());
        e.push(5);
        e.extend_from_slice(b"a.csv");
        m.on_write(vec![packet(&e), packet(&[0x15, 0, 0, 0, 0, 0, 0])]);
        let mut st = [0u8; 64];
        st[0] = 0x18;
        st[20] = 1;
        m.on_write(vec![st]);
        let mut lg = Logger::new(m, false);
        let (files, sd) = lg.sd_list().unwrap();
        assert_eq!(files, vec![SdFile { name: "a.csv".into(), size: 100 }]);
        assert_eq!(sd, Some(1));
    }

    impl Logger<Mock> {
        fn c_written(&self, i: usize) -> Report {
            self.c.transport().written[i]
        }
    }
}
