//! Key=value views of the logger's configuration blocks, shared by the CLI
//! (`get` / `set KEY=VALUE`) and the GUI forms. Values are always metric.

use crate::hid::Report;
use crate::model::*;
use crate::units;
use anyhow::{anyhow, bail, ensure, Context, Result};
use std::collections::BTreeMap;

pub type Kv = BTreeMap<String, String>;

/// How a value may be edited.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Text,
    /// Text that the GUI masks.
    Secret,
    /// One of (value, label).
    Choice(Vec<(String, String)>),
    ReadOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub key: String,
    pub value: String,
    pub kind: Kind,
}

/// One printed line: `k=v  k=v    # note`. A row without fields is a comment.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub fields: Vec<Field>,
    pub note: Option<String>,
}

impl Row {
    fn one(key: &str, value: impl Into<String>, kind: Kind) -> Self {
        Row { label: key.into(), fields: vec![field(key, value, kind)], note: None }
    }

    fn note(mut self, n: impl Into<String>) -> Self {
        self.note = Some(n.into());
        self
    }

    pub fn to_line(&self) -> String {
        let kv: Vec<String> = self.fields.iter().map(|f| format!("{}={}", f.key, f.value)).collect();
        match (&self.note, kv.is_empty()) {
            (Some(n), true) => format!("# {n}"),
            (Some(n), false) => format!("{}    # {n}", kv.join("  ")),
            (None, _) => kv.join("  "),
        }
    }
}

fn field(key: &str, value: impl Into<String>, kind: Kind) -> Field {
    Field { key: key.into(), value: value.into(), kind }
}

fn choices(names: &[&str]) -> Kind {
    Kind::Choice(names.iter().map(|n| (n.to_string(), n.to_string())).collect())
}

fn on_off() -> Kind {
    choices(&["on", "off"])
}

// ------------------------------------------------------------------ parsing

pub fn parse_kv(values: &[String]) -> Result<Kv> {
    let mut m = BTreeMap::new();
    for v in values {
        let (k, val) = v
            .split_once('=')
            .ok_or_else(|| anyhow!("expected KEY=VALUE, got {v:?}"))?;
        m.insert(k.trim().to_ascii_lowercase().replace('-', "_"), val.trim().to_string());
    }
    Ok(m)
}

pub fn parse_bool(s: &str) -> Result<bool> {
    match s.to_ascii_lowercase().as_str() {
        "1" | "on" | "yes" | "true" | "enable" | "enabled" => Ok(true),
        "0" | "off" | "no" | "false" | "disable" | "disabled" => Ok(false),
        _ => bail!("expected on/off, got {s:?}"),
    }
}

/// Decimal string -> tenths, rounded (the app's validators round).
pub fn tenths(s: &str) -> Result<i64> {
    let v: f64 = s.parse().with_context(|| format!("not a number: {s:?}"))?;
    Ok((v * 10.0).round() as i64)
}

pub fn is_unset(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "none" | "unset" | "--")
}

fn unknown_keys(kv: &Kv, known: &[&str]) -> Result<()> {
    for k in kv.keys() {
        ensure!(known.contains(&k.as_str()), "unknown key {k:?}; valid keys: {}", known.join(", "));
    }
    Ok(())
}

pub fn name_of(names: &[&str], i: u8) -> String {
    names.get(i as usize).map(|s| s.to_string()).unwrap_or_else(|| format!("?{i}"))
}

pub fn server_index(s: &str) -> Result<u8> {
    let s = s.to_ascii_lowercase();
    if let Ok(i) = s.parse::<u8>() {
        ensure!(i < 4, "server index must be 0..3");
        return Ok(i);
    }
    SERVERS
        .iter()
        .position(|n| n.starts_with(&s) || (s == "wu" && *n == "wunderground"))
        .map(|i| i as u8)
        .ok_or_else(|| anyhow!("unknown server {s:?}; use one of {}", SERVERS.join(", ")))
}

/// Hex sensor ID, or `learn` to re-register.
pub fn parse_sensor_id(slot: u8, id: &str) -> Result<u32> {
    ensure!(
        SENSOR_SLOTS.iter().any(|(s, _)| *s == slot),
        "unknown slot {slot}; valid: 0 2 3 6 14 22 31 40"
    );
    if id.eq_ignore_ascii_case("learn") {
        return Ok(SENSOR_LEARNING);
    }
    let v = u32::from_str_radix(id.trim().trim_start_matches("0x"), 16)
        .context("ID must be hex (max 8 digits) or `learn`")?;
    ensure!(v < SENSOR_DISABLED, "reserved ID value");
    Ok(v)
}

pub const RAIN_KEYS: &[&str] = &["day", "week", "month", "year", "total"];

/// Rain counter overwrite packet; values in mm, omitted counters unchanged.
pub fn rain_packet(kv: &Kv) -> Result<Report> {
    ensure!(!kv.is_empty(), "give at least one of day= week= month= year= total= (mm)");
    for k in kv.keys() {
        ensure!(RAIN_KEYS.contains(&k.as_str()), "unknown key {k}");
    }
    let get = |k: &str, max: u32| -> Result<Option<u32>> {
        kv.get(k)
            .map(|v| {
                let r = tenths(v)?;
                ensure!((0..=max as i64).contains(&r), "{k} out of range");
                Ok(r as u32)
            })
            .transpose()
    };
    Ok(encode_rain(
        get("day", 9999)?.map(|v| v as u16),
        get("week", 99999)?,
        get("month", 99999)?,
        get("year", 99999)?,
        get("total", 99999)?,
    ))
}

// ----------------------------------------------------------------- settings

pub fn settings_rows(s: &Settings) -> Vec<Row> {
    let u = &s.units;
    let sensor = |v: u8, n: &str| if v == 0 { n.to_string() } else { "none".to_string() };
    let outdoor = OUTDOOR_SENSORS
        .iter()
        .find(|(v, _)| *v == s.outdoor_sensor)
        .map(|x| x.1)
        .unwrap_or("?");
    let zones = TIMEZONES
        .iter()
        .enumerate()
        .map(|(i, (_, dst, name))| (i.to_string(), format!("{name}{}", if *dst { " (DST)" } else { "" })))
        .collect();
    vec![
        Row::one("temp_unit", name_of(units::TEMP_NAMES, u.temp), choices(units::TEMP_NAMES)),
        Row::one("pressure_unit", name_of(units::PRESSURE_NAMES, u.pressure), choices(units::PRESSURE_NAMES)),
        Row::one("wind_unit", name_of(units::WIND_NAMES, u.wind), choices(units::WIND_NAMES)),
        Row::one("rain_unit", name_of(units::RAIN_NAMES, u.rain), choices(units::RAIN_NAMES)),
        Row::one("light_unit", name_of(units::LIGHT_NAMES, u.light), choices(units::LIGHT_NAMES)),
        Row::one("timezone", s.timezone.to_string(), Kind::Choice(zones)).note(timezone_name(s.timezone)),
        Row::one("dst", on(s.dst), on_off()),
        Row::one("interval", (s.interval_s / 60).to_string(), Kind::Text).note("minutes"),
        Row::one("outdoor_sensor", outdoor, choices(&["wh24", "wh65", "none"])),
        Row::one("indoor_sensor", sensor(s.indoor_sensor, "wh30"), choices(&["wh30", "none"])),
        Row::one("soil_sensor", sensor(s.soil_sensor, "wh51"), choices(&["wh51", "none"])),
        Row::one("pm25_sensor", sensor(s.pm25_sensor, "wh41"), choices(&["wh41", "none"])),
        Row::one("area", name_of(AREAS, s.area), choices(AREAS)),
        Row::one("frequency", name_of(FREQUENCIES, s.frequency), Kind::ReadOnly).note("read-only"),
        Row::one("name", s.name.clone(), Kind::Text),
    ]
}

pub fn apply_settings(s: &mut Settings, kv: &Kv) -> Result<()> {
    unknown_keys(kv, &[
        "temp_unit", "pressure_unit", "wind_unit", "rain_unit", "light_unit", "timezone", "dst",
        "interval", "outdoor_sensor", "indoor_sensor", "soil_sensor", "pm25_sensor", "area", "name",
    ])?;
    let unit = |names: &[&str], k: &str, v: &str| {
        units::parse_unit(names, v).ok_or_else(|| anyhow!("{k}: use one of {}", names.join(", ")))
    };
    let opt_sensor = |v: &str, n: &str| -> Result<u8> {
        if v.eq_ignore_ascii_case(n) {
            Ok(0)
        } else if is_unset(v) || v == "off" {
            Ok(0xFF)
        } else {
            bail!("use {n} or none")
        }
    };
    for (k, v) in kv {
        match k.as_str() {
            "temp_unit" => s.units.temp = unit(units::TEMP_NAMES, k, v)?,
            "pressure_unit" => s.units.pressure = unit(units::PRESSURE_NAMES, k, v)?,
            "wind_unit" => s.units.wind = unit(units::WIND_NAMES, k, v)?,
            "rain_unit" => s.units.rain = unit(units::RAIN_NAMES, k, v)?,
            "light_unit" => s.units.light = unit(units::LIGHT_NAMES, k, v)?,
            "timezone" => {
                let i: u8 = v.parse().context("timezone is an index; see `wh6006 timezones`")?;
                ensure!((i as usize) < TIMEZONES.len(), "timezone index must be 0..107");
                s.timezone = i;
            }
            "dst" => s.dst = parse_bool(v)?,
            "interval" => {
                let m: u16 = v.parse().context("interval is in whole minutes")?;
                ensure!((1..=240).contains(&m), "Data Logger Interval: 1 - 240 Minute");
                s.interval_s = m * 60;
            }
            "outdoor_sensor" => {
                s.outdoor_sensor = match v.to_ascii_lowercase().as_str() {
                    "wh24" => 0,
                    "wh65" => 1,
                    x if is_unset(x) => 0xFF,
                    _ => bail!("outdoor_sensor: use wh24, wh65 or none"),
                }
            }
            "indoor_sensor" => s.indoor_sensor = opt_sensor(v, "wh30")?,
            "soil_sensor" => s.soil_sensor = opt_sensor(v, "wh51")?,
            "pm25_sensor" => s.pm25_sensor = opt_sensor(v, "wh41")?,
            "area" => s.area = units::parse_unit(AREAS, v).ok_or_else(|| anyhow!("area: EU or USA"))?,
            "name" => s.name = v.clone(),
            _ => unreachable!(),
        }
    }
    Ok(())
}

// -------------------------------------------------------------- calibration

pub fn calibration_rows(c: &Calibration) -> Vec<Row> {
    let t = |v: f64| format!("{v:.1}");
    let g = |v: u8| format!("{:.2}", v as f64 / 100.0);
    vec![
        Row::one("in_temp", t(c.in_temp as f64 / 10.0), Kind::Text).note("°C offset"),
        Row::one("out_temp", t(c.out_temp as f64 / 10.0), Kind::Text),
        Row::one("in_humi", c.in_humi.to_string(), Kind::Text).note("% offset"),
        Row::one("out_humi", c.out_humi.to_string(), Kind::Text),
        Row::one("abs_pressure", t(c.abs_pressure as f64 / 10.0), Kind::Text).note("hPa offset"),
        Row::one("rel_pressure", t(c.rel_pressure as f64 / 10.0), Kind::Text),
        Row::one("wind_dir", c.wind_dir.to_string(), Kind::Text).note("degrees"),
        Row::one("wind_gain", g(c.wind_gain), Kind::Text),
        Row::one("rain_gain", g(c.rain_gain), Kind::Text),
        Row::one("light_gain", g(c.light_gain), Kind::Text),
    ]
}

pub fn apply_calibration(c: &mut Calibration, kv: &Kv) -> Result<()> {
    unknown_keys(kv, &[
        "in_temp", "out_temp", "in_humi", "out_humi", "abs_pressure", "rel_pressure", "wind_dir",
        "wind_gain", "rain_gain", "light_gain",
    ])?;
    let int = |v: &str| -> Result<i64> { v.parse::<i64>().with_context(|| format!("not an integer: {v:?}")) };
    let gain = |v: &str| -> Result<u8> {
        let g: f64 = v.parse().with_context(|| format!("not a number: {v:?}"))?;
        ensure!((0.5..=1.5).contains(&g), "gain must be 0.50..1.50");
        Ok((g * 100.0).round() as u8)
    };
    for (k, v) in kv {
        match k.as_str() {
            "in_temp" => c.in_temp = tenths(v)?.clamp(-128, 127) as i8,
            "out_temp" => c.out_temp = tenths(v)?.clamp(-128, 127) as i8,
            "in_humi" => c.in_humi = int(v)?.clamp(-128, 127) as i8,
            "out_humi" => c.out_humi = int(v)?.clamp(-128, 127) as i8,
            "abs_pressure" => c.abs_pressure = tenths(v)?.clamp(-32768, 32767) as i16,
            "rel_pressure" => c.rel_pressure = tenths(v)?.clamp(-32768, 32767) as i16,
            "wind_dir" => c.wind_dir = int(v)?.clamp(-32768, 32767) as i16,
            "wind_gain" => c.wind_gain = gain(v)?,
            "rain_gain" => c.rain_gain = gain(v)?,
            "light_gain" => c.light_gain = gain(v)?,
            _ => unreachable!(),
        }
    }
    c.validate()
}

// ------------------------------------------------------------------- alarms

fn fmt_t16(v: i16) -> String {
    if v == 0x7FFF { "none".into() } else { format!("{:.1}", v as f64 / 10.0) }
}
fn fmt_u16(v: u16) -> String {
    if v == 0xFFFF { "none".into() } else { format!("{:.1}", v as f64 / 10.0) }
}
fn fmt_u8(v: u8) -> String {
    if v == 0xFF { "none".into() } else { v.to_string() }
}
pub fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn hi_lo_row(k: &str, high: String, high_on: bool, low: String, low_on: bool, unit: &str) -> Row {
    Row {
        label: k.into(),
        fields: vec![
            field(&format!("{k}_high"), high, Kind::Text),
            field(&format!("{k}_high_on"), on(high_on), on_off()),
            field(&format!("{k}_low"), low, Kind::Text),
            field(&format!("{k}_low_on"), on(low_on), on_off()),
        ],
        note: Some(unit.into()),
    }
}

pub fn alarms_rows(a: &Alarms) -> Vec<Row> {
    let mut rows = Vec::new();
    for (k, v) in [("in_temp", &a.in_temp), ("out_temp", &a.out_temp), ("dew_point", &a.dew_point)] {
        rows.push(hi_lo_row(k, fmt_t16(v.high.value), v.high.enabled, fmt_t16(v.low.value), v.low.enabled, "°C"));
    }
    for (k, v) in [("in_humi", &a.in_humi), ("out_humi", &a.out_humi)] {
        rows.push(hi_lo_row(k, fmt_u8(v.high.value), v.high.enabled, fmt_u8(v.low.value), v.low.enabled, "%"));
    }
    for (k, v, unit) in [("wind", &a.wind, "m/s"), ("gust", &a.gust, "m/s"), ("rain_rate", &a.rain_rate, "mm/h"), ("rain_day", &a.rain_day, "mm")] {
        rows.push(Row {
            label: k.into(),
            fields: vec![field(k, fmt_u16(v.value), Kind::Text), field(&format!("{k}_on"), on(v.enabled), on_off())],
            note: Some(unit.into()),
        });
    }
    rows.push(match a.master {
        Some(m) => Row::one("master", on(m), on_off()),
        None => Row { label: "master".into(), fields: vec![], note: Some("master alarm switch not supported by this firmware".into()) },
    });
    rows
}

pub fn apply_alarms(a: &mut Alarms, kv: &Kv) -> Result<()> {
    let temp = |v: &str| -> Result<i16> {
        if is_unset(v) {
            return Ok(0x7FFF);
        }
        let t = tenths(v)?;
        ensure!((-400..=600).contains(&t), "Alarm Temperature: -40.0 to 60.0 °C");
        Ok(t as i16)
    };
    let humi = |v: &str| -> Result<u8> {
        if is_unset(v) {
            return Ok(0xFF);
        }
        let h: u8 = v.parse().context("humidity is a whole %")?;
        ensure!((1..=99).contains(&h), "Alarm Humidity: 1 to 99");
        Ok(h)
    };
    let pos = |v: &str, max: i64, what: &str| -> Result<u16> {
        if is_unset(v) {
            return Ok(0xFFFF);
        }
        let t = tenths(v)?;
        ensure!((0..=max).contains(&t), "{what} out of range");
        Ok(t as u16)
    };
    for (k, v) in kv {
        let key = k.as_str();
        let (base, rest) = ["in_temp", "out_temp", "dew_point", "in_humi", "out_humi", "rain_rate", "rain_day", "wind", "gust", "master"]
            .iter()
            .find_map(|b| key.strip_prefix(b).map(|r| (*b, r)))
            .ok_or_else(|| anyhow!("unknown alarm key {k:?} (see `alarms get`)"))?;
        match (base, rest) {
            ("master", "") => {
                ensure!(a.master.is_some(), "this firmware has no master alarm switch");
                a.master = Some(parse_bool(v)?);
            }
            ("in_temp" | "out_temp" | "dew_point", r) => {
                let hl = match base {
                    "in_temp" => &mut a.in_temp,
                    "out_temp" => &mut a.out_temp,
                    _ => &mut a.dew_point,
                };
                match r {
                    "_high" => hl.high.value = temp(v)?,
                    "_low" => hl.low.value = temp(v)?,
                    "_high_on" => hl.high.enabled = parse_bool(v)?,
                    "_low_on" => hl.low.enabled = parse_bool(v)?,
                    _ => bail!("unknown alarm key {k:?}"),
                }
            }
            ("in_humi" | "out_humi", r) => {
                let hl = if base == "in_humi" { &mut a.in_humi } else { &mut a.out_humi };
                match r {
                    "_high" => hl.high.value = humi(v)?,
                    "_low" => hl.low.value = humi(v)?,
                    "_high_on" => hl.high.enabled = parse_bool(v)?,
                    "_low_on" => hl.low.enabled = parse_bool(v)?,
                    _ => bail!("unknown alarm key {k:?}"),
                }
            }
            (_, r) => {
                let (t, max) = match base {
                    "wind" => (&mut a.wind, 500),
                    "gust" => (&mut a.gust, 500),
                    "rain_rate" => (&mut a.rain_rate, 9999),
                    _ => (&mut a.rain_day, 9999),
                };
                match r {
                    "" => t.value = pos(v, max, base)?,
                    "_on" => t.enabled = parse_bool(v)?,
                    _ => bail!("unknown alarm key {k:?}"),
                }
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------- mobile

fn fmt_interval(s: u32) -> String {
    if s == 0xFFFF_FFFF {
        "none".into()
    } else {
        (s / 60).to_string()
    }
}

pub fn mobile_rows(m: &Mobile) -> Vec<Row> {
    let alarm = if m.sms_alarm_interval_s == 0xFFFF { "none".into() } else { (m.sms_alarm_interval_s / 60).to_string() };
    let mut rows = vec![
        Row::one("apn", m.apn.clone(), Kind::Text),
        Row::one("user", m.user.clone(), Kind::Text),
        Row::one("password", m.password.clone(), Kind::Secret),
        Row::one("sms", on(m.sms_enabled), on_off()),
        Row::one("alarm_interval_min", alarm, Kind::Text),
        Row::one("send_interval_min", fmt_interval(m.sms_send_interval_s), Kind::Text),
    ];
    for (i, p) in m.phones.iter().enumerate() {
        rows.push(Row::one(&format!("phone{}", i + 1), p.clone(), Kind::Text));
    }
    rows
}

pub fn apply_mobile(m: &mut Mobile, kv: &Kv) -> Result<()> {
    unknown_keys(kv, &["apn", "user", "password", "sms", "alarm_interval_min", "send_interval_min", "phone1", "phone2", "phone3"])?;
    for (k, v) in kv {
        match k.as_str() {
            "apn" => m.apn = v.clone(),
            "user" => m.user = v.clone(),
            "password" => m.password = v.clone(),
            "sms" => m.sms_enabled = parse_bool(v)?,
            "alarm_interval_min" => {
                let n: u16 = v.parse()?;
                ensure!(n <= 120, "Alarm Interval: 0 - 120 Minute");
                m.sms_alarm_interval_s = n * 60;
            }
            "send_interval_min" => {
                let n: u32 = v.parse()?;
                ensure!(n <= 24 * 60, "Send Current Interval: 0 - 24 Hour");
                ensure!(n < 60 || n.is_multiple_of(60), "intervals of an hour or more must be whole hours");
                m.sms_send_interval_s = n * 60;
            }
            p => {
                let i = (p.as_bytes()[5] - b'1') as usize;
                ensure!(v.len() <= 17, "Tel length max 17");
                m.phones[i] = v.clone();
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------- upload

/// Settings the app keeps once and sends identically to all four servers.
pub const UPLOAD_SHARED: &[&str] = &["enabled", "interval_min"];
pub const SERVER_LABELS: &[&str] = &["Wunderground", "Weathercloud", "WOW (Met Office)", "Custom server"];

/// One server's raw values (CLI `upload get`, for diagnosis).
pub fn upload_server_rows(u: &UploadServer) -> Vec<Row> {
    let host_kind = if u.sub == 3 { Kind::Text } else { Kind::ReadOnly };
    vec![
        Row::one("enabled", on(u.enabled()), on_off()),
        Row::one("host", u.host.clone(), host_kind),
        Row::one("port", u.port.to_string(), Kind::Text),
        Row::one("interval_min", fmt_interval(u.interval_s), Kind::Text),
        Row::one("station_id", u.station_id.clone(), Kind::Text),
        Row::one("password", u.password.clone(), Kind::Secret),
        Row::one("type", name_of(SERVER_TYPES, u.server_type), choices(SERVER_TYPES)),
    ]
}

/// The whole upload table: shared switch and interval, then per-server keys
/// prefixed with the server name (`custom.host`). A row without fields and
/// without a note is a heading.
pub fn upload_rows(all: &[UploadServer]) -> Vec<Row> {
    let Some(first) = all.first() else { return Vec::new() };
    let differs = all.iter().any(|u| u.enabled() != first.enabled() || u.interval_s != first.interval_s);
    let mixed = if differs { "; the logger has mixed values, saving unifies them" } else { "" };
    // The app shows its "Disable upload / Enable upload" dropdown only under
    // the www.ecowitt.net entry, but sends that one value in all four packets.
    let mut rows = vec![
        Row::one("interval_min", fmt_interval(first.interval_s), Kind::Text).note(format!("minutes, all services{mixed}")),
        Row { label: "www.ecowitt.net".into(), fields: vec![], note: None },
        Row::one("enabled", on(first.enabled()), on_off()).note(format!("sent with every service{mixed}")),
    ];
    for u in all {
        let name = SERVERS[u.sub as usize];
        let key = |k: &str| format!("{name}.{k}");
        rows.push(Row { label: SERVER_LABELS[u.sub as usize].into(), fields: vec![], note: None });
        let custom = u.sub == 3;
        let host_kind = if custom { Kind::Text } else { Kind::ReadOnly };
        let mut host = Row::one(&key("host"), u.host.clone(), host_kind);
        if custom {
            host = host.note("host name only, no http:// or path");
        }
        rows.push(host);
        rows.push(Row::one(&key("port"), u.port.to_string(), Kind::Text));
        rows.push(Row::one(&key("station_id"), u.station_id.clone(), Kind::Text));
        rows.push(Row::one(&key("password"), u.password.clone(), Kind::Secret));
        if custom {
            rows.push(Row::one(&key("type"), name_of(SERVER_TYPES, u.server_type), choices(SERVER_TYPES)).note("upload path ending"));
        }
    }
    rows
}

/// Apply `enabled`, `interval_min` and `<server>.<key>` changes to the
/// whole table, ready for `set_upload_servers`.
pub fn apply_uploads(all: &mut [UploadServer], kv: &Kv) -> Result<()> {
    ensure!(all.len() == 4, "need the four upload servers");
    let mut per: [Kv; 4] = Default::default();
    for (k, v) in kv {
        match k.split_once('.') {
            Some((srv, key)) => {
                ensure!(!UPLOAD_SHARED.contains(&key), "{key} applies to all servers; give it without a server prefix");
                per[server_index(srv)? as usize].insert(key.to_string(), v.clone());
            }
            None => {
                ensure!(UPLOAD_SHARED.contains(&k.as_str()), "unknown key {k:?}; use enabled, interval_min or <server>.<key>");
                per.iter_mut().for_each(|p| {
                    p.insert(k.clone(), v.clone());
                });
            }
        }
    }
    for (u, kv) in all.iter_mut().zip(&per) {
        apply_upload(u, kv)?;
    }
    // The app sends one global enable flag and interval to every server.
    let (flag, interval) = (all[0].flags & 1, all[0].interval_s);
    for u in all.iter_mut() {
        u.flags = (u.flags & !1) | flag;
        u.interval_s = interval;
    }
    Ok(())
}

/// `https://example.com/` -> `example.com`. Paths and ports are refused;
/// empty is allowed (custom server unused).
fn clean_host(v: &str) -> Result<String> {
    let h = v.trim();
    let h = h.strip_prefix("http://").or_else(|| h.strip_prefix("https://")).unwrap_or(h);
    let h = h.trim_end_matches('/');
    ensure!(!h.contains('/'), "host must be a host name only, without a path: {v:?}");
    ensure!(!h.contains(':'), "give the port in the port field, not in the host: {v:?}");
    ensure!(!h.contains(' '), "invalid host {v:?}");
    Ok(h.to_string())
}

fn apply_upload(u: &mut UploadServer, kv: &Kv) -> Result<()> {
    unknown_keys(kv, &["enabled", "host", "port", "interval_min", "station_id", "password", "type"])?;
    for (k, v) in kv {
        match k.as_str() {
            "enabled" => u.flags = (u.flags & !1) | parse_bool(v)? as u8,
            "host" => {
                ensure!(u.sub == 3, "only the custom server's host can be changed");
                u.host = clean_host(v)?;
            }
            "port" => {
                let p: u16 = v.parse().context("port must be 1..65535")?;
                ensure!(p > 0, "port must be 1..65535");
                u.port = p;
            }
            "interval_min" => {
                let n: u32 = v.parse()?;
                ensure!(n <= 24 * 60, "Update Interval: 0 - 24 Hour");
                ensure!(n < 60 || n.is_multiple_of(60), "intervals of an hour or more must be whole hours");
                u.interval_s = n * 60;
            }
            "station_id" => u.station_id = v.clone(),
            "password" => u.password = v.clone(),
            "type" => {
                u.server_type = units::parse_unit(SERVER_TYPES, v).ok_or_else(|| anyhow!("type: jsp, asp or php"))?
            }
            _ => unreachable!(),
        }
    }
    // The app always forces the well-known hosts.
    if (u.sub as usize) < SERVER_HOSTS.len() {
        u.host = SERVER_HOSTS[u.sub as usize].to_string();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Vec<UploadServer> {
        (0..4u8)
            .map(|sub| UploadServer {
                sub,
                host: SERVER_HOSTS.get(sub as usize).unwrap_or(&"").to_string(),
                flags: 1,
                port: 80,
                interval_s: 300,
                station_id: format!("id{sub}"),
                password: format!("pw{sub}"),
                ..Default::default()
            })
            .collect()
    }

    fn kv(pairs: &[(&str, &str)]) -> Kv {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn custom_edit_leaves_other_servers_alone() {
        let before = table();
        let mut all = before.clone();
        apply_uploads(&mut all, &kv(&[("custom.host", "https://wx.example.org/"), ("custom.station_id", "me"), ("custom.port", "8080")])).unwrap();
        assert_eq!(all[..3], before[..3]);
        assert_eq!(all[3].host, "wx.example.org");
        assert_eq!(all[3].station_id, "me");
        assert_eq!(all[3].port, 8080);
        assert_eq!(all[3].password, "pw3");
    }

    #[test]
    fn shared_values_go_to_every_server() {
        let mut all = table();
        all[2].flags = 0;
        all[3].interval_s = 60;
        apply_uploads(&mut all, &kv(&[("interval_min", "10")])).unwrap();
        assert!(all.iter().all(|u| u.interval_s == 600 && u.enabled()));
        apply_uploads(&mut all, &kv(&[("enabled", "off")])).unwrap();
        assert!(all.iter().all(|u| !u.enabled()));
    }

    #[test]
    fn host_validation() {
        let mut all = table();
        assert!(apply_uploads(&mut all, &kv(&[("custom.host", "example.org/path")])).is_err());
        assert!(apply_uploads(&mut all, &kv(&[("custom.host", "example.org:8080")])).is_err());
        assert!(apply_uploads(&mut all, &kv(&[("wu.host", "evil.example")])).is_err());
        assert!(apply_uploads(&mut all, &kv(&[("wu.enabled", "on")])).is_err());
    }

    #[test]
    fn rows_round_trip_without_changes() {
        let before = table();
        let mut all = before.clone();
        let rows = upload_rows(&all);
        let unchanged: Kv = rows.iter().flat_map(|r| &r.fields).filter(|f| f.kind != Kind::ReadOnly).map(|f| (f.key.clone(), f.value.clone())).collect();
        apply_uploads(&mut all, &unchanged).unwrap();
        assert_eq!(all, before);
    }
}
