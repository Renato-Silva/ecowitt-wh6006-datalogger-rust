//! Background thread that owns the USB connection. Device requests block for
//! up to a few seconds, so the UI only sends `Cmd`s and reads `Reply`s.

use anyhow::{Result, anyhow};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;
use wh6006::device::Logger;
use wh6006::firmware;
use wh6006::form::{self, Kv, Row};
use wh6006::hid::HidDevice;
use wh6006::model::*;
use wh6006::units::Units;
use wh6006::view::{self, LatestFirmware};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Settings,
    Calibration,
    Alarms,
    Mobile,
    Upload,
}

pub enum Cmd {
    Connect,
    Info,
    Live,
    MaxMin,
    ClearMaxMin,
    Load(Section),
    Save(Section, Kv),
    SetRain(Kv),
    SyncTime,
    SyncNetTime,
    TestSms,
    Sensors,
    SetSensor(u8, String),
    SdList,
    SdDownload(Vec<SdFile>, PathBuf),
    SdDelete(String),
    Reboot,
    FactoryReset,
    CheckUpdates,
    FirmwareUpdate(PathBuf),
}

pub enum Reply {
    Connected { version: Version, name: String, units: Units },
    Disconnected,
    Info { version: Version, name: String, status: Status },
    Live(Live),
    Units(Units),
    MaxMin(Vec<MaxMinItem>),
    Rows(Section, Vec<Row>),
    Sensors(Vec<SensorRecord>),
    Sd(Vec<SdFile>, Option<u8>),
    Updates { latest: LatestFirmware, device: Option<Version> },
    Progress(String, f32),
    /// Device log text (0x89 packets), raw chunks.
    Log(Vec<String>),
    Ok(String),
    Err(String),
    /// Sent after every command, successful or not.
    Done,
}

pub struct Worker {
    pub path: Option<PathBuf>,
    pub debug: bool,
    pub tx: Sender<Reply>,
    pub ctx: eframe::egui::Context,
    lg: Option<Logger<HidDevice>>,
}

impl Worker {
    pub fn new(path: Option<PathBuf>, debug: bool, tx: Sender<Reply>, ctx: eframe::egui::Context) -> Self {
        Self { path, debug, tx, ctx, lg: None }
    }

    pub fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(cmd) => {
                    let res = self.handle(cmd);
                    self.flush_log();
                    if let Err(e) = res {
                        self.failed(&e);
                        self.send(Reply::Err(format!("{e:#}")));
                    }
                    self.send(Reply::Done);
                }
                // While idle, keep reading so log packets arrive live.
                Err(RecvTimeoutError::Timeout) => {
                    let Some(lg) = self.lg.as_mut() else { continue };
                    let res = lg.c.drain();
                    self.flush_log();
                    if let Err(e) = res {
                        self.failed(&e);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    /// An I/O error means the device went away; reopen next time.
    fn failed(&mut self, e: &anyhow::Error) {
        if e.chain().any(|c| c.is::<std::io::Error>()) && self.lg.take().is_some() {
            self.send(Reply::Disconnected);
        }
    }

    fn flush_log(&mut self) {
        let Some(lg) = self.lg.as_mut() else { return };
        let lines = lg.c.take_log();
        if !lines.is_empty() {
            self.send(Reply::Log(lines));
        }
    }

    fn send(&self, r: Reply) {
        let _ = self.tx.send(r);
        self.ctx.request_repaint();
    }

    fn connect(&mut self) -> Result<()> {
        self.lg = None;
        let dev = HidDevice::open_logger(self.path.as_deref())?;
        let mut lg = Logger::new(dev, self.debug);
        match lg.ensure_app_mode() {
            Ok(version) => {
                let name = lg.name().unwrap_or_default();
                let units = lg.settings().map(|s| s.units).unwrap_or(Units::METRIC);
                self.lg = Some(lg);
                self.send(Reply::Connected { version, name, units });
                Ok(())
            }
            Err(e) => {
                // Stay connected in bootloader mode so firmware can be flashed.
                if let Ok(version) = lg.version()
                    && version.is_bootloader()
                {
                    self.lg = Some(lg);
                    self.send(Reply::Connected { version, name: String::new(), units: Units::METRIC });
                }
                Err(e)
            }
        }
    }

    fn lg(&mut self) -> Result<&mut Logger<HidDevice>> {
        if self.lg.is_none() {
            self.connect()?;
        }
        self.lg.as_mut().ok_or_else(|| anyhow!("not connected"))
    }

    fn load(&mut self, section: Section, kv: Option<&Kv>) -> Result<Vec<Row>> {
        let lg = self.lg()?;
        let rows = match section {
            Section::Settings => {
                if let Some(kv) = kv {
                    let mut s = lg.settings()?;
                    form::apply_settings(&mut s, kv)?;
                    lg.set_settings(&s)?;
                }
                let s = lg.settings()?;
                self.send(Reply::Units(s.units));
                form::settings_rows(&s)
            }
            Section::Calibration => {
                if let Some(kv) = kv {
                    let mut c = lg.calibration()?;
                    form::apply_calibration(&mut c, kv)?;
                    lg.set_calibration(&c)?;
                }
                form::calibration_rows(&lg.calibration()?)
            }
            Section::Alarms => {
                if let Some(kv) = kv {
                    let mut a = lg.alarms()?;
                    form::apply_alarms(&mut a, kv)?;
                    lg.set_alarms(&a)?;
                }
                form::alarms_rows(&lg.alarms()?)
            }
            Section::Mobile => {
                if let Some(kv) = kv {
                    let mut m = lg.mobile()?;
                    form::apply_mobile(&mut m, kv)?;
                    lg.set_mobile(&m)?;
                }
                form::mobile_rows(&lg.mobile()?)
            }
            Section::Upload => {
                if let Some(kv) = kv {
                    let mut all = lg.upload_servers()?;
                    form::apply_uploads(&mut all, kv)?;
                    lg.set_upload_servers(&all)?;
                }
                form::upload_rows(&lg.upload_servers()?)
            }
        };
        Ok(rows)
    }

    fn handle(&mut self, cmd: Cmd) -> Result<()> {
        match cmd {
            Cmd::Connect => self.connect()?,
            Cmd::Info => {
                let lg = self.lg()?;
                let version = lg.ensure_app_mode()?;
                let name = lg.name().unwrap_or_default();
                let status = lg.status()?;
                self.send(Reply::Info { version, name, status });
            }
            Cmd::Live => {
                let l = self.lg()?.live()?;
                self.send(Reply::Live(l));
            }
            Cmd::MaxMin => {
                let m = self.lg()?.maxmin()?;
                self.send(Reply::MaxMin(m));
            }
            Cmd::ClearMaxMin => {
                self.lg()?.clear_maxmin()?;
                let m = self.lg()?.maxmin()?;
                self.send(Reply::MaxMin(m));
                self.send(Reply::Ok("Max/min memory cleared".into()));
            }
            Cmd::Load(section) => {
                let rows = self.load(section, None)?;
                self.send(Reply::Rows(section, rows));
            }
            Cmd::Save(section, kv) => {
                let rows = self.load(section, Some(&kv))?;
                self.send(Reply::Rows(section, rows));
                self.send(Reply::Ok("Saved".into()));
            }
            Cmd::SetRain(kv) => {
                let p = form::rain_packet(&kv)?;
                self.lg()?.set_rain(p)?;
                self.send(Reply::Ok("Rain counters saved".into()));
            }
            Cmd::SyncTime => {
                self.lg()?.sync_time_utc()?;
                self.send(Reply::Ok("Clock set from this computer".into()));
            }
            Cmd::SyncNetTime => {
                self.lg()?.sync_net_time()?;
                self.send(Reply::Ok("Asked the logger to fetch network time".into()));
            }
            Cmd::TestSms => {
                self.lg()?.test_sms()?;
                self.send(Reply::Ok("Test message requested".into()));
            }
            Cmd::Sensors => {
                let lg = self.lg()?;
                let mut all = Vec::new();
                for (slot, _) in SENSOR_SLOTS {
                    all.push(lg.sensor(*slot)?);
                }
                self.send(Reply::Sensors(all));
            }
            Cmd::SetSensor(slot, id) => {
                let id = form::parse_sensor_id(slot, &id)?;
                self.lg()?.set_sensor_id(slot, id)?;
                self.send(Reply::Ok("Sensor ID saved".into()));
                self.handle(Cmd::Sensors)?;
            }
            Cmd::SdList => {
                let (files, sd) = self.lg()?.sd_list()?;
                self.send(Reply::Sd(files, sd));
            }
            Cmd::SdDownload(files, dir) => {
                std::fs::create_dir_all(&dir)?;
                let tx = self.tx.clone();
                let ctx = self.ctx.clone();
                let lg = self.lg()?;
                for f in &files {
                    let dest = view::safe_join(&dir, &f.name)?;
                    lg.sd_download(f, &dest, false, |got, total| {
                        let _ = tx.send(Reply::Progress(f.name.clone(), got as f32 / total.max(1) as f32));
                        ctx.request_repaint();
                    })?;
                }
                self.send(Reply::Ok(format!("Downloaded {} file(s) to {}", files.len(), dir.display())));
            }
            Cmd::SdDelete(name) => {
                self.lg()?.sd_delete(&name)?;
                self.send(Reply::Ok(format!("Deleted {name}")));
                self.handle(Cmd::SdList)?;
            }
            Cmd::Reboot => {
                self.lg()?.reset_device()?;
                self.send(Reply::Ok("Logger rebooted".into()));
            }
            Cmd::FactoryReset => {
                self.lg()?.factory_reset()?;
                self.send(Reply::Ok("Factory settings restored".into()));
                let units = self.lg()?.settings()?.units;
                self.send(Reply::Units(units));
            }
            Cmd::CheckUpdates => {
                let latest = view::latest_firmware()?;
                let device = self.lg().ok().and_then(|lg| lg.version().ok());
                self.send(Reply::Updates { latest, device });
            }
            Cmd::FirmwareUpdate(hex) => {
                let img = firmware::load_hex(&hex)?;
                let path = self.path.clone();
                let tx = self.tx.clone();
                let ctx = self.ctx.clone();
                let lg = self.lg()?;
                let mut reopen = || HidDevice::open_logger(path.as_deref());
                let mut last = (String::new(), usize::MAX);
                firmware::update(lg, &img, &mut reopen, &mut |stage, i, n| {
                    let pct = i * 100 / n.max(1);
                    if (stage, pct) != (last.0.as_str(), last.1) {
                        last = (stage.to_string(), pct);
                        let _ = tx.send(Reply::Progress(stage.to_string(), pct as f32 / 100.0));
                        ctx.request_repaint();
                    }
                })?;
                self.send(Reply::Ok("Firmware updated".into()));
                self.connect()?;
            }
        }
        Ok(())
    }
}
