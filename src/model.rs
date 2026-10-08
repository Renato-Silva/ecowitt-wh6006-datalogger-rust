//! Typed encode/decode of every WH6006 command payload.
//!
//! Raw values are kept in device units (0.1 °C, 0.1 hPa, 0.1 m/s, 0.1 mm,
//! 0.1 lux, %) so a read-modify-write round trip is lossless.

use crate::hid::Report;
use crate::protocol::packet;
use anyhow::{bail, ensure, Result};
use serde::Serialize;

pub fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub fn i16le(b: &[u8], o: usize) -> i16 {
    u16le(b, o) as i16
}
pub fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Read a `len, bytes...` string at `*pos`, advancing it.
fn take_str(b: &[u8], pos: &mut usize) -> Result<String> {
    ensure!(*pos < b.len(), "truncated string length at {}", *pos);
    let n = b[*pos] as usize;
    let start = *pos + 1;
    ensure!(start + n <= b.len(), "string of length {n} overruns packet");
    *pos = start + n;
    Ok(String::from_utf8_lossy(&b[start..start + n]).into_owned())
}

fn put_str(out: &mut Vec<u8>, s: &str) -> Result<()> {
    ensure!(s.is_ascii(), "only ASCII text is supported: {s:?}");
    ensure!(s.len() <= 255, "string too long: {s:?}");
    out.push(s.len() as u8);
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

/// Build a packet from `body` (which starts at payload[0]) checking it fits.
fn finish(body: Vec<u8>) -> Result<Report> {
    ensure!(
        body.len() <= 64,
        "packet would be {} bytes, the limit is 64 (shorten the text fields)",
        body.len()
    );
    Ok(packet(&body))
}

// ---------------------------------------------------------------------------
// 0x80 version
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Version {
    /// "Firmware Ver" or "Bootloader Ver"
    pub label: String,
    pub version: String,
}

impl Version {
    pub fn decode(r: &Report) -> Result<Self> {
        let n = (r[2] as usize).min(61);
        let text = String::from_utf8_lossy(&r[3..3 + n]).into_owned();
        let (label, version) = text
            .split_once(':')
            .map(|(a, b)| (a.to_string(), b.trim().to_string()))
            .unwrap_or((text.clone(), String::new()));
        Ok(Self { label, version })
    }
    pub fn is_bootloader(&self) -> bool {
        self.label.contains("Bootloader")
    }
    /// Comparison key used by the original app: digits with dots removed.
    pub fn numeric(v: &str) -> u32 {
        v.chars()
            .filter(|c| *c != '.')
            .collect::<String>()
            .trim()
            .parse()
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// 0x8f sub 1: live data block (244 bytes)
// ---------------------------------------------------------------------------

pub const LIVE_LEN: usize = 244;

#[derive(Debug, Clone, Serialize)]
pub struct Live {
    /// Device clock, local time encoded as seconds since 1970.
    pub device_time: u32,
    pub learning_countdown_s: u16,
    /// Percentage shown in the status bar (probably battery).
    pub battery_pct: u8,
    pub sim_present: bool,
    /// 1=2G 2=3G 3=4G 4=5G
    pub network: u8,
    pub signal: u8,
    pub in_temp: i16,
    pub abs_pressure: u16,
    pub rel_pressure: u16,
    pub in_humi: u8,
    pub out_humi: u8,
    pub out_temp: i16,
    pub dew_point: i16,
    pub wind_chill: i16,
    pub heat_index: i16,
    pub wind: u16,
    pub gust: u16,
    pub wind_dir: u16,
    pub light: u32,
    pub pm25: u16,
    pub soil_moisture: u8,
    pub extra_temp: i16,
    pub leaf_wetness: u8,
    pub rain_hour: u16,
    pub rain_event: u16,
    pub rain_day: u16,
    pub rain_week: u32,
    pub rain_month: u32,
    pub rain_year: u32,
    pub rain_total: u32,
    pub rain_rate: u16,
    pub uv_raw: u16,
    pub uvi: u8,
}

impl Live {
    pub fn decode(b: &[u8]) -> Result<Self> {
        ensure!(b.len() >= LIVE_LEN, "live block is {} bytes, need {LIVE_LEN}", b.len());
        Ok(Self {
            device_time: u32le(b, 0x00),
            learning_countdown_s: u16le(b, 0x08),
            battery_pct: b[0x0a],
            sim_present: b[0x0c] != 0xFF,
            network: b[0x0d],
            signal: b[0x0e],
            in_temp: i16le(b, 0x10),
            abs_pressure: u16le(b, 0x12),
            rel_pressure: u16le(b, 0x14),
            in_humi: b[0x16],
            out_humi: b[0x17],
            out_temp: i16le(b, 0x18),
            dew_point: i16le(b, 0x1a),
            wind_chill: i16le(b, 0x1c),
            heat_index: i16le(b, 0x1e),
            wind: u16le(b, 0x22),
            gust: u16le(b, 0x24),
            wind_dir: u16le(b, 0x28),
            light: u32le(b, 0x2c),
            pm25: u16le(b, 0x34),
            soil_moisture: b[0x44],
            extra_temp: i16le(b, 0x64),
            leaf_wetness: b[0x74],
            rain_hour: u16le(b, 0x90),
            rain_event: u16le(b, 0xc0),
            rain_day: u16le(b, 0xc2),
            rain_week: u32le(b, 0xc4),
            rain_month: u32le(b, 0xc8),
            rain_year: u32le(b, 0xcc),
            rain_total: u32le(b, 0xd0),
            rain_rate: u16le(b, 0xd4),
            uv_raw: u16le(b, 0xd8),
            uvi: b[0xda],
        })
    }
}

/// u16 rain field: 0xFFFF = no data.
pub fn rain16(v: u16) -> Option<u32> {
    (v != 0xFFFF).then_some(v as u32)
}
/// u32 rain field: 0xFFFFFFFF = no data.
pub fn rain32(v: u32) -> Option<u32> {
    (v != 0xFFFF_FFFF).then_some(v)
}

// ---------------------------------------------------------------------------
// 0x8f sub 2 / 0x91: sensor slots
// ---------------------------------------------------------------------------

/// (slot, sensor name) in the order the app shows them.
pub const SENSOR_SLOTS: &[(u8, &str)] = &[
    (0, "Outdoor array (WH65/WH24)"),
    (2, "Wind (WH80)"),
    (3, "Rain (WH40)"),
    (6, "Indoor T/H (WH31 CH1)"),
    (14, "Soil moisture (WH51 CH1)"),
    (22, "PM2.5 (WH41 CH1)"),
    (31, "Extra temperature (WH34 CH1)"),
    (40, "Leaf wetness (WH35 CH1)"),
];

pub const SENSOR_LEARNING: u32 = 0xFFFF_FFFF;
pub const SENSOR_DISABLED: u32 = 0xFFFF_FFFE;

#[derive(Debug, Clone, Serialize)]
pub struct SensorRecord {
    pub slot: u8,
    pub signal: u8,
    pub battery: u8,
    pub id: u32,
    pub raw: Vec<u8>,
}

impl SensorRecord {
    pub fn decode(slot: u8, b: &[u8]) -> Result<Self> {
        ensure!(b.len() >= 8, "sensor record too short");
        Ok(Self {
            slot,
            signal: b[1],
            battery: b[2],
            id: u32le(b, 4),
            raw: b.to_vec(),
        })
    }
    pub fn id_text(&self) -> String {
        match self.id {
            SENSOR_LEARNING => "Learning".into(),
            SENSOR_DISABLED => "Disabled".into(),
            id => format!("{id:X}"),
        }
    }
}

pub fn encode_sensor_id(slot: u8, id: u32) -> Report {
    let mut b = vec![0x91, 0x07, slot];
    b.extend_from_slice(&id.to_le_bytes());
    packet(&b)
}

// ---------------------------------------------------------------------------
// 0x18 status / diagnostics
// ---------------------------------------------------------------------------

pub const GSM_STATES: &[&str] = &[
    "GSM_NORMAL", "GSM_CGSN", "GSM_ATI", "GSM_QCFG1", "GSM_QCFG2", "GSM_QCFG3", "GSM_QCFG4",
    "GSM_QSCLK", "GSM_CPIN", "GSM_QINISTAT", "GSM_CSQ", "GSM_CREG", "GSM_COPS", "GSM_CFUN0",
    "GSM_CFUN1", "GSM_CGREG", "GSM_QGBAND", "GSM_QISEND", "GSM_CMGF", "GSM_CSCS", "GSM_CSMP",
    "GSM_CMGSTEL", "GSM_CPMS", "GSM_CMGR", "GSM_CMGD", "GSM_QICSGP", "GSM_QICLOSE",
    "GSM_QIDEACT", "GSM_POWERON", "GSM_SENDSMS", "GSM_QIACT", "GSM_DNS", "GSM_TCPSEND",
];

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub current_task: u32,
    pub wait_task: u32,
    pub gsm_state: u8,
    pub gsm_auto_restarts: u8,
    pub gsm_error_restarts: u8,
    pub upload_ok_wu: u8,
    pub upload_ok_wcld: u8,
    pub upload_ok_wow: u8,
    pub upload_ok_user: u8,
    pub connect_fail_wu: u8,
    pub connect_fail_wcld: u8,
    pub sd_status: u8,
    pub connect_fail_wow: u8,
    pub connect_fail_user: u8,
    pub connect_fail_time: u8,
    pub battery_volts: f64,
}

impl Status {
    pub fn decode(r: &Report) -> Self {
        Self {
            current_task: u32le(r, 3),
            wait_task: u32le(r, 7),
            gsm_state: r[11],
            gsm_auto_restarts: r[12],
            gsm_error_restarts: r[13],
            upload_ok_wu: r[14],
            upload_ok_wcld: r[15],
            upload_ok_wow: r[16],
            upload_ok_user: r[17],
            connect_fail_wu: r[18],
            connect_fail_wcld: r[19],
            sd_status: r[20],
            connect_fail_wow: r[21],
            connect_fail_user: r[22],
            connect_fail_time: r[23],
            battery_volts: u16le(r, 24) as f64 / 10000.0,
        }
    }
    pub fn gsm_state_name(&self) -> &'static str {
        GSM_STATES.get(self.gsm_state as usize).copied().unwrap_or("?")
    }
    pub fn sd_status_name(&self) -> &'static str {
        sd_status_name(self.sd_status)
    }
}

pub fn sd_status_name(v: u8) -> &'static str {
    match v {
        0 => "SD Initializing",
        1 => "Normal",
        2 => "No SD Card",
        3 => "Mount Failed",
        _ => "Open Failed",
    }
}

// ---------------------------------------------------------------------------
// 0x0f max/min (TLV over two packets), 0x10 clear
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum MmValue {
    Temp(i16),
    Humi(u8),
    Pressure(u16),
    Wind(u16),
    Rain(u16),
    Light(u32),
    U16(u16),
    U8(u8),
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MaxMinItem {
    pub tag: u8,
    pub name: &'static str,
    pub max: Option<MmValue>,
    pub min: Option<MmValue>,
}

pub fn decode_maxmin(d: &[u8]) -> Vec<MaxMinItem> {
    use MmValue::*;
    let mut out = Vec::new();
    let mut pos = 0;
    // The app parses while pos < 0x43.
    while pos < d.len().min(0x43) {
        let tag = d[pos];
        let rest = &d[pos + 1..];
        let need = match tag {
            0x01 | 0x02 | 0x03 | 0x08 | 0x15 | 0x19 | 0x22 => 4,
            0x04 | 0x05 | 0x06 | 0x07 | 0x0b | 0x0c | 0x0e | 0x0f | 0x10 | 0x16 => 2,
            0x17 | 0x23 => 1,
            _ => break,
        };
        if rest.len() < need {
            break;
        }
        let (name, max, min) = match tag {
            0x01 => ("Indoor temperature", Some(Temp(i16le(rest, 0))), Some(Temp(i16le(rest, 2)))),
            0x02 => ("Outdoor temperature", Some(Temp(i16le(rest, 0))), Some(Temp(i16le(rest, 2)))),
            0x03 => ("Dew point", Some(Temp(i16le(rest, 0))), Some(Temp(i16le(rest, 2)))),
            0x04 => ("Wind chill", None, Some(Temp(i16le(rest, 0)))),
            0x05 => ("Heat index", Some(Temp(i16le(rest, 0))), None),
            0x06 => ("Indoor humidity", Some(Humi(rest[0])), Some(Humi(rest[1]))),
            0x07 => ("Outdoor humidity", Some(Humi(rest[0])), Some(Humi(rest[1]))),
            0x08 => ("Absolute pressure", Some(Pressure(u16le(rest, 0))), Some(Pressure(u16le(rest, 2)))),
            0x0b => ("Wind", Some(Wind(u16le(rest, 0))), None),
            0x0c => ("Gust", Some(Wind(u16le(rest, 0))), None),
            0x0e => ("Rain rate", Some(Rain(u16le(rest, 0))), None),
            0x0f => ("Rain hour", Some(Rain(u16le(rest, 0))), None),
            0x10 => ("Rain day", Some(Rain(u16le(rest, 0))), None),
            0x15 => ("Light", Some(Light(u32le(rest, 0))), None),
            0x16 => ("UV (raw)", Some(U16(u16le(rest, 0))), None),
            0x17 => ("UVI", Some(U8(rest[0])), None),
            0x19 => ("Unknown temperature (0x19)", Some(Temp(i16le(rest, 0))), Some(Temp(i16le(rest, 2)))),
            0x22 => ("Soil moisture", Some(U16(u16le(rest, 0))), Some(U16(u16le(rest, 2)))),
            0x23 => ("PM2.5", Some(U8(rest[0])), None),
            _ => unreachable!(),
        };
        out.push(MaxMinItem { tag, name, max, min });
        pos += 1 + need;
    }
    out
}

/// Constant clear packet captured from the app (0x4210c0).
pub const CLEAR_MAXMIN: &[u8] = &[
    0x10, 0x2d, 0x01, 0x00, 0x00, 0x02, 0x00, 0x00, 0x06, 0x00, 0x00, 0x07, 0x00, 0x00, 0x03,
    0x00, 0x00, 0x08, 0x00, 0x00, 0x05, 0x00, 0x04, 0x00, 0x0b, 0x00, 0x0c, 0x00, 0x0e, 0x00,
    0x0f, 0x00, 0x10, 0x00, 0x15, 0x00, 0x16, 0x00, 0x17, 0x00, 0x22, 0x00, 0x00, 0x23, 0x00,
];

// ---------------------------------------------------------------------------
// 0x05 / 0x06 basic settings
// ---------------------------------------------------------------------------

pub const OUTDOOR_SENSORS: &[(u8, &str)] = &[(0, "wh24"), (1, "wh65"), (0xFF, "none")];
pub const FREQUENCIES: &[&str] = &["915MHz", "868MHz", "433MHz", "920MHz", "RS485"];
pub const AREAS: &[&str] = &["EU", "USA"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Settings {
    pub units: crate::units::Units,
    pub timezone: u8,
    pub dst: bool,
    pub interval_s: u16,
    pub outdoor_sensor: u8,
    pub indoor_sensor: u8,
    pub soil_sensor: u8,
    pub pm25_sensor: u8,
    pub area: u8,
    pub frequency: u8,
    pub name: String,
}

impl Settings {
    pub fn decode(r: &Report) -> Result<Self> {
        let mut pos = 17;
        let name = take_str(r, &mut pos)?;
        Ok(Self {
            units: crate::units::Units {
                temp: r[2],
                pressure: r[3],
                wind: r[4],
                rain: r[5],
                light: r[6],
            },
            timezone: r[7],
            dst: r[8] != 0,
            interval_s: u16le(r, 9),
            outdoor_sensor: r[11],
            indoor_sensor: r[12],
            soil_sensor: r[13],
            pm25_sensor: r[14],
            area: r[15],
            frequency: r[16],
            name,
        })
    }

    /// cmd 0x06. Unlike the original (which always wrote 0 for the frequency
    /// byte) we echo back whatever was read, so a round trip changes nothing.
    pub fn encode(&self) -> Result<Report> {
        let u = &self.units;
        let mut b = vec![
            0x06,
            0, // length, filled below
            u.temp,
            u.pressure,
            u.wind,
            u.rain,
            u.light,
            self.timezone,
            self.dst as u8,
        ];
        b.extend_from_slice(&self.interval_s.to_le_bytes());
        b.extend_from_slice(&[
            self.outdoor_sensor,
            self.indoor_sensor,
            self.soil_sensor,
            self.pm25_sensor,
            self.area,
            self.frequency,
        ]);
        ensure!(self.name.len() <= 31, "device name must be at most 31 characters");
        put_str(&mut b, &self.name)?;
        b[1] = b.len() as u8; // == 0x12 + name_len
        finish(b)
    }
}

/// cmd 0x02 tag 0x18: set clock. The app sends UTC (GetSystemTime).
pub fn encode_time(year: u16, month: u8, day: u8, h: u8, m: u8, s: u8) -> Report {
    let y = year.to_be_bytes();
    packet(&[0x02, 0x0a, 0x18, y[0], y[1], month, day, h, m, s])
}

/// cmd 0x02 tags 0x10..0x14: set rain counters (0.1 mm). `None` sends the
/// all-ones sentinel, which (inferred) leaves the counter unchanged.
pub fn encode_rain(day: Option<u16>, week: Option<u32>, month: Option<u32>, year: Option<u32>, total: Option<u32>) -> Report {
    let mut b = vec![0x02, 0x19, 0x10];
    b.extend_from_slice(&day.unwrap_or(0xFFFF).to_le_bytes());
    for (tag, v) in [(0x11, week), (0x12, month), (0x13, year), (0x14, total)] {
        b.push(tag);
        b.extend_from_slice(&v.unwrap_or(0xFFFF_FFFF).to_le_bytes());
    }
    debug_assert_eq!(b.len(), 0x19);
    packet(&b)
}

// ---------------------------------------------------------------------------
// 0x03 / 0x04 calibration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Calibration {
    /// 0.1 °C, ±60
    pub in_temp: i8,
    pub out_temp: i8,
    /// % RH, ±50
    pub in_humi: i8,
    pub out_humi: i8,
    /// 0.1 hPa, ±8000
    pub abs_pressure: i16,
    pub rel_pressure: i16,
    /// degrees, ±180
    pub wind_dir: i16,
    /// ×0.01, 50..150
    pub wind_gain: u8,
    pub rain_gain: u8,
    pub light_gain: u8,
}

impl Calibration {
    pub fn decode(r: &Report) -> Result<Self> {
        let mut c = Calibration::default();
        let mut pos = 2;
        // The app walks items while the offset (from p[2]) is < 0x17.
        while pos < 2 + 0x17 {
            let tag = r[pos];
            match tag {
                0x01 => c.in_temp = r[pos + 1] as i8,
                0x02 => c.out_temp = r[pos + 1] as i8,
                0x06 => c.in_humi = r[pos + 1] as i8,
                0x07 => c.out_humi = r[pos + 1] as i8,
                0x08 => c.abs_pressure = i16le(r, pos + 1),
                0x09 => c.rel_pressure = i16le(r, pos + 1),
                0x0a => c.wind_dir = i16le(r, pos + 1),
                0x0b => c.wind_gain = r[pos + 1],
                0x0d => c.rain_gain = r[pos + 1],
                0x15 => c.light_gain = r[pos + 1],
                _ => bail!("unexpected calibration tag 0x{tag:02x} at byte {pos}"),
            }
            pos += if matches!(tag, 0x08..=0x0a) { 3 } else { 2 };
        }
        Ok(c)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!((-60..=60).contains(&self.in_temp) && (-60..=60).contains(&self.out_temp), "temperature offset must be within ±6.0 °C");
        ensure!((-50..=50).contains(&self.in_humi) && (-50..=50).contains(&self.out_humi), "humidity offset must be within ±50 %");
        ensure!((-8000..=8000).contains(&self.abs_pressure) && (-8000..=8000).contains(&self.rel_pressure), "pressure offset must be within ±800 hPa");
        ensure!((-180..=180).contains(&self.wind_dir), "wind direction offset must be within ±180°");
        for g in [self.wind_gain, self.rain_gain, self.light_gain] {
            ensure!((50..=150).contains(&g), "gains must be within 0.50..1.50");
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Report> {
        self.validate()?;
        let mut b = vec![0x04, 0x19];
        b.extend_from_slice(&[0x01, self.in_temp as u8, 0x02, self.out_temp as u8]);
        b.extend_from_slice(&[0x06, self.in_humi as u8, 0x07, self.out_humi as u8]);
        for (tag, v) in [(0x08, self.abs_pressure), (0x09, self.rel_pressure), (0x0a, self.wind_dir)] {
            b.push(tag);
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&[0x0b, self.wind_gain, 0x0d, self.rain_gain, 0x15, self.light_gain]);
        debug_assert_eq!(b.len(), 0x19);
        finish(b)
    }
}

// ---------------------------------------------------------------------------
// 0x0d / 0x0e alarms
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Default)]
pub struct Threshold<T> {
    pub value: T,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct HiLo<T> {
    pub high: Threshold<T>,
    pub low: Threshold<T>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Alarms {
    /// 0.1 °C; 0x7FFF = unset
    pub in_temp: HiLo<i16>,
    pub out_temp: HiLo<i16>,
    pub dew_point: HiLo<i16>,
    /// %; 0xFF = unset
    pub in_humi: HiLo<u8>,
    pub out_humi: HiLo<u8>,
    /// 0.1 m/s
    pub wind: Threshold<u16>,
    pub gust: Threshold<u16>,
    /// 0.1 mm/h
    pub rain_rate: Threshold<u16>,
    /// 0.1 mm
    pub rain_day: Threshold<u16>,
    /// `Some` if the device supports the master alarm item (reply len 0x33).
    pub master: Option<bool>,
}

impl Alarms {
    pub fn decode(r: &Report) -> Result<Self> {
        let mut a = Alarms::default();
        let end = (r[1] as usize).clamp(2, 64);
        let mut pos = 2;
        let t16 = |o: usize| Threshold { value: i16le(r, o), enabled: r[o + 2] != 0 };
        let t8 = |o: usize| Threshold { value: r[o], enabled: r[o + 1] != 0 };
        let u16t = |o: usize| Threshold { value: u16le(r, o), enabled: r[o + 2] != 0 };
        while pos < end {
            let tag = r[pos];
            let p = pos + 1;
            let size = match tag {
                0x01..=0x03 => {
                    let v = HiLo { high: t16(p), low: t16(p + 3) };
                    match tag {
                        0x01 => a.in_temp = v,
                        0x02 => a.out_temp = v,
                        _ => a.dew_point = v,
                    }
                    7
                }
                0x06 | 0x07 => {
                    let v = HiLo { high: t8(p), low: t8(p + 2) };
                    if tag == 0x06 { a.in_humi = v } else { a.out_humi = v }
                    5
                }
                0x0b | 0x0c | 0x0e | 0x10 => {
                    let v = u16t(p);
                    match tag {
                        0x0b => a.wind = v,
                        0x0c => a.gust = v,
                        0x0e => a.rain_rate = v,
                        _ => a.rain_day = v,
                    }
                    4
                }
                0x21 => {
                    a.master = Some(r[p] != 0);
                    2
                }
                _ => bail!("unexpected alarm tag 0x{tag:02x} at byte {pos}"),
            };
            pos += size;
        }
        Ok(a)
    }

    pub fn encode(&self) -> Result<Report> {
        let mut b = vec![0x0e, 0];
        let hl16 = |b: &mut Vec<u8>, tag: u8, v: &HiLo<i16>| {
            b.push(tag);
            b.extend_from_slice(&v.high.value.to_le_bytes());
            b.push(v.high.enabled as u8);
            b.extend_from_slice(&v.low.value.to_le_bytes());
            b.push(v.low.enabled as u8);
        };
        let hl8 = |b: &mut Vec<u8>, tag: u8, v: &HiLo<u8>| {
            b.extend_from_slice(&[tag, v.high.value, v.high.enabled as u8, v.low.value, v.low.enabled as u8]);
        };
        let one = |b: &mut Vec<u8>, tag: u8, v: &Threshold<u16>| {
            b.push(tag);
            b.extend_from_slice(&v.value.to_le_bytes());
            b.push(v.enabled as u8);
        };
        // Order exactly as the app's save handler (0x41c420).
        hl16(&mut b, 0x01, &self.in_temp);
        hl8(&mut b, 0x06, &self.in_humi);
        hl16(&mut b, 0x02, &self.out_temp);
        hl8(&mut b, 0x07, &self.out_humi);
        hl16(&mut b, 0x03, &self.dew_point);
        one(&mut b, 0x0b, &self.wind);
        one(&mut b, 0x0c, &self.gust);
        one(&mut b, 0x0e, &self.rain_rate);
        one(&mut b, 0x10, &self.rain_day);
        if let Some(m) = self.master {
            b.extend_from_slice(&[0x21, m as u8]);
        }
        b[1] = b.len() as u8; // 0x31, or 0x33 with the master item
        finish(b)
    }
}

// ---------------------------------------------------------------------------
// Mobile network: 0x11/0x12 APN, 0x13/0x14 user+pass, 0x07/0x08 SMS
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Mobile {
    pub apn: String,
    pub user: String,
    pub password: String,
    /// seconds
    pub sms_alarm_interval_s: u16,
    /// seconds
    pub sms_send_interval_s: u32,
    pub sms_enabled: bool,
    pub phones: [String; 3],
}

impl Mobile {
    pub fn decode(apn: &Report, cred: &Report, sms: &Report) -> Result<Self> {
        let mut p = 2;
        let apn_s = take_str(apn, &mut p)?;
        let mut p = 2;
        let user = take_str(cred, &mut p)?;
        let password = take_str(cred, &mut p)?;
        let mut p = 9;
        let t1 = take_str(sms, &mut p)?;
        let t2 = take_str(sms, &mut p)?;
        let t3 = take_str(sms, &mut p)?;
        Ok(Self {
            apn: apn_s,
            user,
            password,
            sms_alarm_interval_s: u16le(sms, 2),
            sms_send_interval_s: u32le(sms, 4),
            sms_enabled: sms[8] != 0,
            phones: [t1, t2, t3],
        })
    }

    /// Returns the three write packets 0x12, 0x14, 0x08 in the app's order.
    pub fn encode(&self) -> Result<[Report; 3]> {
        let mut a = vec![0x12, 0];
        put_str(&mut a, &self.apn)?;
        // Faithful to the app: p[1] = n + 2 although the packet is n + 3 bytes.
        a[1] = (self.apn.len() + 2) as u8;

        let mut c = vec![0x14, 0];
        put_str(&mut c, &self.user)?;
        put_str(&mut c, &self.password)?;
        c[1] = c.len() as u8;

        let mut s = vec![0x08, 0];
        s.extend_from_slice(&self.sms_alarm_interval_s.to_le_bytes());
        s.extend_from_slice(&self.sms_send_interval_s.to_le_bytes());
        s.push(self.sms_enabled as u8);
        for t in &self.phones {
            ensure!(t.len() <= 17, "phone numbers are limited to 17 characters");
            put_str(&mut s, t)?;
        }
        s[1] = s.len() as u8;
        Ok([finish(a)?, finish(c)?, finish(s)?])
    }
}

pub const TEST_SMS: &str = "Weather Logger Test MSG";

pub fn encode_test_sms() -> Report {
    let mut b = vec![0x8a, (TEST_SMS.len() + 2) as u8];
    b.extend_from_slice(TEST_SMS.as_bytes());
    packet(&b)
}

// ---------------------------------------------------------------------------
// Upload servers: 0x09/0x0a host, 0x0b/0x0c parameters (sub 0..3)
// ---------------------------------------------------------------------------

pub const SERVERS: &[&str] = &["wunderground", "weathercloud", "wow", "custom"];
pub const SERVER_HOSTS: &[&str] = &[
    "rtupdate.wunderground.com",
    "api.weathercloud.net",
    "wow.metoffice.gov.uk",
];
pub const SERVER_TYPES: &[&str] = &["jsp", "asp", "php"];

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct UploadServer {
    pub sub: u8,
    pub host: String,
    /// 0=jsp 1=asp 2=php (custom server path style)
    pub server_type: u8,
    /// Raw flags byte; bit 0 = upload enabled.
    pub flags: u8,
    pub port: u16,
    pub interval_s: u32,
    pub station_id: String,
    pub password: String,
}

impl UploadServer {
    pub fn enabled(&self) -> bool {
        self.flags & 1 != 0
    }

    pub fn decode(host: &Report, params: &Report) -> Result<Self> {
        let mut p = 3;
        let host_s = take_str(host, &mut p)?;
        let mut p = 11;
        let id = take_str(params, &mut p)?;
        let pw = take_str(params, &mut p)?;
        Ok(Self {
            sub: params[2],
            host: host_s,
            server_type: params[3],
            flags: params[4],
            port: u16le(params, 5),
            interval_s: u32le(params, 7),
            station_id: id,
            password: pw,
        })
    }

    pub fn encode(&self) -> Result<[Report; 2]> {
        let mut h = vec![0x0a, 0, self.sub];
        put_str(&mut h, &self.host)?;
        h[1] = h.len() as u8;

        let mut p = vec![0x0c, 0, self.sub, self.server_type, self.flags];
        p.extend_from_slice(&self.port.to_le_bytes());
        p.extend_from_slice(&self.interval_s.to_le_bytes());
        put_str(&mut p, &self.station_id)?;
        put_str(&mut p, &self.password)?;
        p[1] = p.len() as u8;
        Ok([finish(h)?, finish(p)?])
    }
}

// ---------------------------------------------------------------------------
// SD card: 0x15 list, 0x16 download, 0x17 delete
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SdFile {
    pub name: String,
    pub size: u32,
}

/// One 0x15 packet. `None` = end-of-list marker (name length 0).
pub fn decode_sd_entry(r: &Report) -> Option<SdFile> {
    let n = r[6] as usize;
    if n == 0 {
        return None;
    }
    let n = n.min(64 - 7);
    Some(SdFile {
        name: String::from_utf8_lossy(&r[7..7 + n]).into_owned(),
        size: u32le(r, 2),
    })
}

pub fn encode_sd_download(name: &str, offset: u32, remaining: u32) -> Result<Report> {
    let mut b = vec![0x16, 0];
    b.extend_from_slice(&offset.to_le_bytes());
    b.extend_from_slice(&remaining.to_le_bytes());
    put_str(&mut b, name)?;
    b[1] = b.len() as u8; // N + 11
    finish(b)
}

pub fn encode_sd_delete(name: &str) -> Result<Report> {
    let mut b = vec![0x17, 0];
    put_str(&mut b, name)?;
    b[1] = b.len() as u8; // N + 3
    finish(b)
}

/// Chunks with fewer than this many data bytes end a download.
pub const SD_FULL_CHUNK: usize = 61;

// ---------------------------------------------------------------------------
// Timezone table (settings byte 7). (UTC offset minutes, has DST, name)
// ---------------------------------------------------------------------------

pub const TIMEZONES: &[(i16, bool, &str)] = &[
    (-720, false, "(UTC-12:00)International Date Line West"),
    (-660, false, "(UTC-11:00)Samoa"),
    (-660, false, "(UTC-11:00)Coordinated Universal Time-11"),
    (-600, true, "(UTC-10:00)Aleutian Islands"),
    (-600, false, "(UTC-10:00)Hawaii"),
    (-570, false, "(UTC-09:30)Marquesas Islands"),
    (-540, true, "(UTC-09:00)Alaska"),
    (-540, false, "(UTC-09:00)Coordinated Universal Time-9"),
    (-480, true, "(UTC-08:00)Pacific Time (US & Canada)"),
    (-480, true, "(UTC-08:00)Baja California"),
    (-480, false, "(UTC-08:00)Coordinated Universal Time-8"),
    (-420, true, "(UTC-07:00)Chihuahua,La Paz,Mazatlan"),
    (-420, true, "(UTC-07:00)Mountain Time (US & Canada)"),
    (-420, false, "(UTC-07:00)Arizona"),
    (-360, true, "(UTC-06:00)Guadalajara,Mexico City,Monterrey"),
    (-360, false, "(UTC-06:00)Saskatchewan"),
    (-360, true, "(UTC-06:00)Central Time (US & Canada)"),
    (-360, false, "(UTC-06:00)Central America"),
    (-300, false, "(UTC-05:00)Bogota,Lima,Quito"),
    (-300, true, "(UTC-05:00)Eastern Time (US & Canada)"),
    (-300, true, "(UTC-05:00)Havana"),
    (-300, true, "(UTC-05:00)Hayti"),
    (-300, false, "(UTC-05:00)Chetumal"),
    (-300, true, "(UTC-05:00)Indiana (East)"),
    (-270, false, "(UTC-04:30)Caracas"),
    (-240, true, "(UTC-04:00)Atlantic Time (Canada)"),
    (-240, true, "(UTC-04:00)Cuiaba"),
    (-240, false, "(UTC-04:00)Georgetown,La Paz,Manaus,San Juan"),
    (-240, true, "(UTC-04:00)Santiago"),
    (-240, true, "(UTC-04:00)Asuncion"),
    (-210, true, "(UTC-03:30)Newfoundland"),
    (-180, true, "(UTC-03:00)Brasilia"),
    (-180, false, "(UTC-03:00)Buenos Aires"),
    (-180, true, "(UTC-03:00)Greenland"),
    (-180, false, "(UTC-03:00)Cayenne,Fortaleza"),
    (-180, false, "(UTC-03:00)Montevideo"),
    (-120, false, "(UTC-02:00)Coordinated Universal Time-02"),
    (-60, false, "(UTC-01:00)Cape Verde Is."),
    (-60, true, "(UTC-01:00)Azores"),
    (0, true, "(UTC)Dublin,Edinburgh,Lisbon,London"),
    (0, true, "(UTC)Casablanca"),
    (0, false, "(UTC)Monrovia,Reykjavik"),
    (0, false, "(UTC)Coordinated Universal Time"),
    (60, true, "(UTC+01:00)Amsterdam,Berlin,Bern,Rome,Stockholm,Vienna"),
    (60, true, "(UTC+01:00)Belgrade,Bratislava,Budapest,Ljubljana,Prague"),
    (60, true, "(UTC+01:00)Brussels,Copenhagen,Madrid,Paris"),
    (60, true, "(UTC+01:00)Sarajevo,Skopje,Warsaw,Zagreb"),
    (60, true, "(UTC+01:00)Windhoek"),
    (60, false, "(UTC+01:00)West Central Africa"),
    (120, true, "(UTC+02:00)Amman"),
    (120, true, "(UTC+02:00)Beirut"),
    (120, true, "(UTC+02:00)Damascus"),
    (120, false, "(UTC+02:00)Harare,Pretoria"),
    (120, true, "(UTC+02:00)Helsinki,Kyiv,Riga,Sofia,Talinn,Vilnius"),
    (120, false, "(UTC+02:00)Cairo"),
    (120, true, "(UTC+02:00)Athens,Bucharest,Istanbul"),
    (120, true, "(UTC+02:00)Jerusalem"),
    (180, false, "(UTC+03:00)Baghdad"),
    (180, false, "(UTC+03:00)Kuwait,Riyadh"),
    (180, false, "(UTC+03:00)Minsk"),
    (180, false, "(UTC+03:00)Moscow,St.Petersburg,Volgograd"),
    (180, false, "(UTC+03:00)Nairobi"),
    (210, true, "(UTC+03:30)Tehran"),
    (240, false, "(UTC+04:00)Abu Dhabi,Muscat"),
    (240, false, "(UTC+04:00)Yerevan"),
    (240, false, "(UTC+04:00)Baku"),
    (240, false, "(UTC+04:00)Tbilisi"),
    (240, false, "(UTC+04:00)Port Louis"),
    (270, false, "(UTC+04:30)Kabul"),
    (300, false, "(UTC+05:00)Tashkent"),
    (300, false, "(UTC+05:00)Ekaterinburg"),
    (300, false, "(UTC+05:00)Islamabad,Karachi"),
    (330, false, "(UTC+05:30)Chennai,Kolkata,Mumbai,New Delhi"),
    (330, false, "(UTC+05:30)Sri Jayawardenepura"),
    (345, false, "(UTC+05:45)Kathmandu"),
    (360, false, "(UTC+06:00)Astana"),
    (360, false, "(UTC+06:00)Dhaka"),
    (360, false, "(UTC+06:00)Novosibirsk"),
    (390, false, "(UTC+06:30)Yangon (Rangoon)"),
    (420, false, "(UTC+07:00)Kobdo"),
    (420, false, "(UTC+07:00)Krasnoyarsk"),
    (420, false, "(UTC+07:00)Bangkok,Hanoi,Jakarta"),
    (480, false, "(UTC+08:00)Beijing,Chongqing,Hong Kong,Urumqi"),
    (480, false, "(UTC+08:00)Kuala Lumpur,Singapore"),
    (480, false, "(UTC+08:00)Perth"),
    (480, false, "(UTC+08:00)Taipei"),
    (480, false, "(UTC+08:00)Ulaanbaatar"),
    (480, false, "(UTC+08:00)Irkutsk"),
    (540, false, "(UTC+09:00)Pyongyang"),
    (540, false, "(UTC+09:00)Osaka,Sapporo,Tokyo"),
    (540, false, "(UTC+09:00)Seoul"),
    (540, false, "(UTC+09:00)Yakutsk"),
    (570, true, "(UTC+09:30)Adelaide"),
    (570, false, "(UTC+09:30)Darwin"),
    (600, false, "(UTC+10:00)Brisbane"),
    (600, false, "(UTC+10:00)Vladivostok"),
    (600, false, "(UTC+10:00)Guam,Port Moresby"),
    (600, true, "(UTC+10:00)Hobart"),
    (600, true, "(UTC+10:00)Canberra,Melbourne,Sydney"),
    (630, true, "(UTC+10:30)Lord Howe Island"),
    (660, false, "(UTC+11:00)Magadan"),
    (660, false, "(UTC+11:00)Solomon Is.,New Caledonia"),
    (720, true, "(UTC+12:00)Auckland,Wellington"),
    (720, true, "(UTC+12:00)Fiji"),
    (720, false, "(UTC+12:00)Coordinated Universal Time+12"),
    (765, true, "(UTC+12:45)Chatham Islands"),
    (780, false, "(UTC+13:00)Nuku'alofa"),
    (840, false, "(UTC+14:00)Christmas Island"),
];

pub fn timezone_name(i: u8) -> &'static str {
    TIMEZONES.get(i as usize).map(|t| t.2).unwrap_or("?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timezone_table_complete() {
        assert_eq!(TIMEZONES.len(), 108);
        assert_eq!(TIMEZONES[42].2, "(UTC)Coordinated Universal Time");
    }

    #[test]
    fn time_packet_matches_spec_example() {
        let p = encode_time(2026, 10, 8, 12, 34, 56);
        assert_eq!(&p[..10], &[0x02, 0x0a, 0x18, 0x07, 0xea, 0x0a, 0x08, 0x0c, 0x22, 0x38]);
    }

    #[test]
    fn settings_round_trip() {
        let mut r = [0u8; 64];
        let name = b"MyStation";
        r[..17].copy_from_slice(&[
            0x05, 0x12 + name.len() as u8, 1, 2, 1, 0, 0, 43, 1, 0x2c, 0x01, 1, 0xff, 0xff, 0xff, 0, 1,
        ]);
        r[17] = name.len() as u8;
        r[18..18 + name.len()].copy_from_slice(name);
        let s = Settings::decode(&r).unwrap();
        assert_eq!(s.interval_s, 300);
        assert_eq!(s.timezone, 43);
        assert_eq!(s.name, "MyStation");
        assert_eq!(s.frequency, 1);
        let w = s.encode().unwrap();
        assert_eq!(w[0], 0x06);
        assert_eq!(&w[1..], &r[1..]);
    }

    #[test]
    fn calibration_round_trip() {
        let c = Calibration {
            in_temp: -5,
            out_temp: 12,
            in_humi: -3,
            out_humi: 4,
            abs_pressure: -120,
            rel_pressure: 250,
            wind_dir: -90,
            wind_gain: 100,
            rain_gain: 105,
            light_gain: 95,
        };
        let mut p = c.encode().unwrap();
        assert_eq!(&p[..2], &[0x04, 0x19]);
        p[0] = 0x03;
        assert_eq!(Calibration::decode(&p).unwrap(), c);
    }

    #[test]
    fn alarm_lengths_and_round_trip() {
        let mut a = Alarms::default();
        a.in_temp.high = Threshold { value: 300, enabled: true };
        a.out_humi.low = Threshold { value: 20, enabled: true };
        a.rain_day = Threshold { value: 500, enabled: false };
        let p = a.encode().unwrap();
        assert_eq!(&p[..2], &[0x0e, 0x31]);
        assert_eq!(Alarms::decode(&p).unwrap(), a);
        a.master = Some(true);
        let p = a.encode().unwrap();
        assert_eq!(p[1], 0x33);
        assert_eq!(Alarms::decode(&p).unwrap(), a);
    }

    #[test]
    fn rain_packet_layout() {
        let p = encode_rain(Some(12), None, Some(1000), None, None);
        assert_eq!(&p[..5], &[0x02, 0x19, 0x10, 12, 0]);
        assert_eq!(&p[5..10], &[0x11, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(&p[10..15], &[0x12, 0xe8, 0x03, 0, 0]);
        assert_eq!(p[25], 0);
    }

    #[test]
    fn mobile_round_trip() {
        let m = Mobile {
            apn: "internet".into(),
            user: "u".into(),
            password: "pw".into(),
            sms_alarm_interval_s: 600,
            sms_send_interval_s: 3600,
            sms_enabled: true,
            phones: ["+351900000000".into(), "".into(), "123".into()],
        };
        let [a, c, s] = m.encode().unwrap();
        assert_eq!(a[1] as usize, m.apn.len() + 2);
        assert_eq!(c[1] as usize, 1 + 2 + 4);
        assert_eq!(s[1] as usize, 13 + 0 + 3 + 12);
        let mut a2 = a;
        a2[0] = 0x11;
        assert_eq!(Mobile::decode(&a2, &c, &s).unwrap(), m);
    }

    #[test]
    fn upload_round_trip() {
        let u = UploadServer {
            sub: 1,
            host: "api.weathercloud.net".into(),
            server_type: 2,
            flags: 1,
            port: 80,
            interval_s: 600,
            station_id: "abc".into(),
            password: "key".into(),
        };
        let [h, p] = u.encode().unwrap();
        assert_eq!(h[1] as usize, u.host.len() + 4);
        assert_eq!(p[1] as usize, 3 + 3 + 13);
        assert_eq!(UploadServer::decode(&h, &p).unwrap(), u);
    }

    #[test]
    fn maxmin_tlv() {
        let d = [
            0x01, 0x2c, 0x01, 0x9c, 0xff, // in temp max 30.0 min -10.0
            0x06, 80, 20, // in humi
            0x15, 0x10, 0x27, 0, 0, // light 10000
            0x17, 7, // UVI
            0x00, // terminator
        ];
        let v = decode_maxmin(&d);
        assert_eq!(v.len(), 4);
        assert_eq!(v[0].max, Some(MmValue::Temp(300)));
        assert_eq!(v[0].min, Some(MmValue::Temp(-100)));
        assert_eq!(v[2].max, Some(MmValue::Light(10000)));
        assert_eq!(v[3].max, Some(MmValue::U8(7)));
    }

    #[test]
    fn clear_maxmin_is_45_bytes() {
        assert_eq!(CLEAR_MAXMIN.len(), 0x2d);
    }

    #[test]
    fn sd_packets() {
        let p = encode_sd_download("20260101.csv", 100, 900).unwrap();
        assert_eq!(p[0], 0x16);
        assert_eq!(p[1] as usize, 12 + 11);
        assert_eq!(&p[2..6], &100u32.to_le_bytes());
        assert_eq!(&p[6..10], &900u32.to_le_bytes());
        assert_eq!(p[10], 12);
        let d = encode_sd_delete("a.csv").unwrap();
        assert_eq!(&d[..8], &[0x17, 8, 5, b'a', b'.', b'c', b's', b'v']);

        let mut e = [0u8; 64];
        e[0] = 0x15;
        e[2..6].copy_from_slice(&1234u32.to_le_bytes());
        e[6] = 5;
        e[7..12].copy_from_slice(b"x.csv");
        assert_eq!(decode_sd_entry(&e), Some(SdFile { name: "x.csv".into(), size: 1234 }));
        e[6] = 0;
        assert_eq!(decode_sd_entry(&e), None);
    }

    #[test]
    fn version_parse() {
        let mut r = [0u8; 64];
        let s = b"Firmware Ver:1.0.5";
        r[0] = 0x80;
        r[2] = s.len() as u8;
        r[3..3 + s.len()].copy_from_slice(s);
        let v = Version::decode(&r).unwrap();
        assert_eq!(v.label, "Firmware Ver");
        assert_eq!(v.version, "1.0.5");
        assert!(!v.is_bootloader());
        assert_eq!(Version::numeric("1.0.5"), 105);
    }

    #[test]
    fn live_decode_offsets() {
        let mut b = vec![0u8; LIVE_LEN];
        b[0x10..0x12].copy_from_slice(&215i16.to_le_bytes());
        b[0x18..0x1a].copy_from_slice(&(-35i16).to_le_bytes());
        b[0x2c..0x30].copy_from_slice(&0x00FF_FFFFu32.to_le_bytes());
        b[0x0c] = 0xFF;
        b[0xda] = 6;
        let l = Live::decode(&b).unwrap();
        assert_eq!(l.in_temp, 215);
        assert_eq!(l.out_temp, -35);
        assert_eq!(l.light, 0x00FF_FFFF);
        assert!(!l.sim_present);
        assert_eq!(l.uvi, 6);
    }
}
