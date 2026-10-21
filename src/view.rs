//! Formatting of read-only data (live readings, status, max/min, SD files),
//! shared by the CLI and the GUI.

use crate::model::*;
use crate::units::{self, Units};
use anyhow::{anyhow, ensure, Context, Result};
use std::path::{Path, PathBuf};

pub fn device_time(l: &Live) -> String {
    chrono::DateTime::from_timestamp(l.device_time as i64, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "?".into())
}

pub fn network_name(l: &Live) -> &'static str {
    match l.network {
        1 => "2G",
        2 => "3G",
        3 => "4G",
        4 => "5G",
        _ => "-",
    }
}

pub fn live_rows(l: &Live, u: &Units) -> Vec<(&'static str, String)> {
    vec![
        ("Indoor temp", u.temp(l.in_temp)),
        ("Indoor humidity", units::pct(l.in_humi)),
        ("Outdoor temp", u.temp(l.out_temp)),
        ("Outdoor humidity", units::pct(l.out_humi)),
        ("Abs pressure", u.pressure(l.abs_pressure)),
        ("Rel pressure", u.pressure(l.rel_pressure)),
        ("Dew point", u.temp(l.dew_point)),
        ("Wind chill", u.temp(l.wind_chill)),
        ("Heat index", u.temp(l.heat_index)),
        ("Wind direction", if l.wind_dir == 0xFFFF { "--".into() } else { format!("{} °", l.wind_dir) }),
        ("Wind", u.wind(l.wind)),
        ("Gust", u.wind(l.gust)),
        ("Rain rate", u.rain(rain16(l.rain_rate), true)),
        ("Rain event", u.rain(rain16(l.rain_event), false)),
        ("Rain hour", u.rain(rain16(l.rain_hour), false)),
        ("Rain day", u.rain(rain16(l.rain_day), false)),
        ("Rain week", u.rain(rain32(l.rain_week), false)),
        ("Rain month", u.rain(rain32(l.rain_month), false)),
        ("Rain year", u.rain(rain32(l.rain_year), false)),
        ("Rain total", u.rain(rain32(l.rain_total), false)),
        ("Light", u.light(l.light)),
        ("UVI", if l.uvi == 0xFF { "--".into() } else { l.uvi.to_string() }),
        ("Soil moisture", units::pct(l.soil_moisture)),
        ("PM2.5", units::pm25(l.pm25)),
        ("Temperature", u.temp(l.extra_temp)),
        ("Leaf wetness", units::pct(l.leaf_wetness)),
    ]
}

pub fn status_rows(s: &Status) -> Vec<(&'static str, String)> {
    vec![
        ("GSM state", s.gsm_state_name().to_string()),
        ("SD card", s.sd_status_name().to_string()),
        ("Battery voltage", format!("{:.4} V", s.battery_volts)),
        ("Current task", format!("0x{:08X}   wait task 0x{:08X}", s.current_task, s.wait_task)),
        ("GSM restarts", format!("auto {}  error {}", s.gsm_auto_restarts, s.gsm_error_restarts)),
        (
            "Uploads OK",
            format!("WU {}  WCLD {}  WOW {}  user {}", s.upload_ok_wu, s.upload_ok_wcld, s.upload_ok_wow, s.upload_ok_user),
        ),
        (
            "Connect fails",
            format!(
                "WU {}  WCLD {}  WOW {}  user {}  time {}",
                s.connect_fail_wu, s.connect_fail_wcld, s.connect_fail_wow, s.connect_fail_user, s.connect_fail_time
            ),
        ),
    ]
}

pub fn fmt_mm(v: &MmValue, u: &Units) -> String {
    match v {
        MmValue::Temp(t) => u.temp(*t),
        MmValue::Humi(h) => units::pct(*h),
        MmValue::Pressure(p) => u.pressure(*p),
        MmValue::Wind(w) => u.wind(*w),
        MmValue::Rain(r) => u.rain(rain16(*r), false),
        MmValue::Light(l) => u.light(*l),
        MmValue::U16(x) => if *x == 0xFFFF { "--".into() } else { x.to_string() },
        MmValue::U8(x) => if *x == 0xFF { "--".into() } else { x.to_string() },
    }
}

pub fn human_size(n: u32) -> String {
    let kb = n as f64 / 1024.0;
    if n <= 0x100000 {
        format!("{kb:.2} KB")
    } else {
        format!("{:.2} MB", kb / 1024.0)
    }
}

/// Refuse path traversal in device-supplied file names.
pub fn safe_join(dir: &Path, name: &str) -> Result<PathBuf> {
    ensure!(
        !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != "..",
        "refusing unsafe file name {name:?}"
    );
    Ok(dir.join(name))
}

// ----------------------------------------------------------- online check

pub struct LatestFirmware {
    pub version: String,
    pub url: String,
    /// One note per line.
    pub notes: String,
}

fn fetch_ini(url: &str) -> Result<String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "20", url])
        .output()
        .context("running curl (is it installed?)")?;
    ensure!(out.status.success(), "download failed: {url}");
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn ini_get(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_sec = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_sec = l.trim_matches(['[', ']']).eq_ignore_ascii_case(section);
            continue;
        }
        if let (true, Some((k, v))) = (in_sec, l.split_once('='))
            && k.trim().eq_ignore_ascii_case(key)
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// Ask Ecowitt's server for the newest WH6006 firmware.
pub fn latest_firmware() -> Result<LatestFirmware> {
    let fw = fetch_ini("http://download.ecowitt.net/down/filewave?v=FirwaveReadme.txt")?;
    let version = ini_get(&fw, "WH6006", "VER").ok_or_else(|| anyhow!("no [WH6006] VER in firmware readme"))?;
    let url = ini_get(&fw, "WH6006", "ULR").unwrap_or_default();
    let notes = ini_get(&fw, "WH6006", "NOTES").unwrap_or_default().replace(';', "\n");
    Ok(LatestFirmware { version, url, notes })
}
