//! wh6006 — Linux command-line replacement for Ecowitt's "Weather Logger"
//! Windows app (v1.7.3) for the WH6006 GSM weather logger.

mod device;
mod firmware;
mod hid;
mod model;
mod protocol;
mod units;

use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::{Parser, Subcommand};
use device::Logger;
use hid::HidDevice;
use model::*;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use units::Units;

#[derive(Parser)]
#[command(version, about = "Talk to an Ecowitt WH6006 weather logger over USB")]
struct Cli {
    /// hidraw node to use (default: auto-detect USB 04d8:003c)
    #[arg(long, global = true)]
    device: Option<PathBuf>,
    /// Hex-dump every packet to stderr
    #[arg(long, global = true)]
    debug: bool,
    /// Print JSON instead of text where supported
    #[arg(long, global = true)]
    json: bool,
    /// Show values in metric units instead of the logger's configured units
    #[arg(long, global = true)]
    metric: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List connected loggers
    Devices,
    /// Firmware version, device name and status
    Info,
    /// Current readings
    Live {
        /// Repeat every N seconds
        #[arg(long, value_name = "N")]
        watch: Option<u64>,
    },
    /// Diagnostics (GSM state, upload counters, SD status, battery)
    Status,
    /// Recorded max/min values
    Maxmin {
        /// Clear the max/min memory
        #[arg(long)]
        clear: bool,
    },
    /// Units, timezone, logging interval, sensors, device name
    Settings {
        #[command(subcommand)]
        op: GetSet,
    },
    /// List the timezone indexes accepted by `settings set timezone=N`
    Timezones,
    /// Set the logger clock
    Time {
        #[command(subcommand)]
        op: TimeOp,
    },
    /// Sensor offsets and gains
    Calibration {
        #[command(subcommand)]
        op: GetSet,
    },
    /// Alarm thresholds
    Alarms {
        #[command(subcommand)]
        op: GetSet,
    },
    /// Overwrite rain counters (values in mm): day= week= month= year= total=
    Rain { values: Vec<String> },
    /// APN, GPRS credentials, SMS settings and phone numbers
    Mobile {
        #[command(subcommand)]
        op: MobileOp,
    },
    /// Weather-service upload settings (wunderground, weathercloud, wow, custom)
    Upload {
        #[command(subcommand)]
        op: UploadOp,
    },
    /// Wireless sensor IDs
    Sensors {
        #[command(subcommand)]
        op: SensorOp,
    },
    /// Files on the logger's SD card
    Sd {
        #[command(subcommand)]
        op: SdOp,
    },
    /// Reboot the logger, or restore factory settings
    Reset {
        /// Factory reset instead of a reboot
        #[arg(long)]
        factory: bool,
        /// Don't ask for confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Firmware tools
    Firmware {
        #[command(subcommand)]
        op: FwOp,
    },
    /// Check Ecowitt's server for newer firmware / PC software
    CheckUpdates,
    /// Send raw bytes (hex) and print the replies. For protocol debugging.
    Raw {
        bytes: Vec<String>,
        /// How many reply packets to wait for
        #[arg(long, default_value_t = 1)]
        count: usize,
        /// Per-packet timeout in milliseconds
        #[arg(long, default_value_t = 2000)]
        wait: u64,
    },
}

#[derive(Subcommand)]
enum GetSet {
    /// Show current values (keys usable with `set`)
    Get,
    /// Change values: KEY=VALUE ...
    Set { values: Vec<String> },
}

#[derive(Subcommand)]
enum TimeOp {
    /// Send the PC clock (UTC; the logger applies its timezone setting)
    Sync,
    /// Ask the logger to fetch time from the network
    SyncNet,
}

#[derive(Subcommand)]
enum MobileOp {
    Get,
    /// KEY=VALUE: apn user password sms=on|off alarm_interval_min send_interval_min phone1..3
    Set { values: Vec<String> },
    /// Send "Weather Logger Test MSG" to the configured phones
    TestSms,
}

#[derive(Subcommand)]
enum UploadOp {
    /// Show one server or all of them
    Get { server: Option<String> },
    /// KEY=VALUE: enabled=on|off host port interval_min station_id password type=jsp|asp|php
    Set { server: String, values: Vec<String> },
}

#[derive(Subcommand)]
enum SensorOp {
    Get,
    /// Set a sensor ID (hex) or `learn` to re-register
    Set { slot: u8, id: String },
}

#[derive(Subcommand)]
enum SdOp {
    /// List files and card status
    List,
    /// Download files (all files with --all)
    Download {
        names: Vec<String>,
        #[arg(long)]
        all: bool,
        /// Output directory
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
        /// Continue a partial download instead of starting over
        #[arg(long)]
        resume: bool,
    },
    /// Delete a file from the card
    Delete {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum FwOp {
    /// Show the firmware/bootloader version
    Version,
    /// Convert an Intel HEX image to the bin layout the app uses (Tempbin.bin)
    Convert { hex: PathBuf, out: PathBuf },
    /// Flash a firmware .hex file
    Update {
        hex: PathBuf,
        #[arg(long)]
        yes: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn open(cli: &Cli) -> Result<Logger<HidDevice>> {
    let dev = HidDevice::open_logger(cli.device.as_deref())?;
    Ok(Logger::new(dev, cli.debug))
}

/// Open and make sure the application firmware (not the bootloader) runs.
fn open_app(cli: &Cli) -> Result<Logger<HidDevice>> {
    let mut lg = open(cli)?;
    lg.ensure_app_mode()?;
    Ok(lg)
}

fn display_units(cli: &Cli, lg: &mut Logger<HidDevice>) -> Units {
    if cli.metric {
        return Units::METRIC;
    }
    lg.settings().map(|s| s.units).unwrap_or(Units::METRIC)
}

fn print_json<T: serde::Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn confirm(prompt: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    eprint!("{prompt} [y/N] ");
    std::io::stderr().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    ensure!(s.trim().eq_ignore_ascii_case("y"), "aborted");
    Ok(())
}

fn run(cli: Cli) -> Result<()> {
    match &cli.cmd {
        Cmd::Devices => {
            let devs = hid::enumerate(Some((hid::VID, hid::PID)))?;
            if devs.is_empty() {
                println!("no loggers found (USB {:04x}:{:04x})", hid::VID, hid::PID);
            }
            for d in devs {
                println!("{}  {:04x}:{:04x}  {}", d.path.display(), d.vid, d.pid, d.name);
            }
        }
        Cmd::Info => {
            let mut lg = open(&cli)?;
            let v = lg.ensure_app_mode()?;
            let name = lg.name().unwrap_or_default();
            println!("{} : {}", v.label, v.version);
            println!("Device name: {name}");
            print_status(&cli, &lg.status()?)?;
        }
        Cmd::Live { watch } => {
            let mut lg = open_app(&cli)?;
            let u = display_units(&cli, &mut lg);
            loop {
                let live = lg.live()?;
                if cli.json {
                    print_json(&live)?;
                } else {
                    print_live(&live, &u);
                }
                let Some(n) = watch else { break };
                std::thread::sleep(Duration::from_secs((*n).max(1)));
                if !cli.json {
                    println!();
                }
            }
        }
        Cmd::Status => {
            let mut lg = open_app(&cli)?;
            print_status(&cli, &lg.status()?)?;
        }
        Cmd::Maxmin { clear } => {
            let mut lg = open_app(&cli)?;
            if *clear {
                lg.clear_maxmin()?;
                println!("Clear Success");
                return Ok(());
            }
            let items = lg.maxmin()?;
            if cli.json {
                return print_json(&items);
            }
            let u = display_units(&cli, &mut lg);
            for it in items {
                let f = |v: &Option<MmValue>| v.as_ref().map(|v| fmt_mm(v, &u)).unwrap_or_default();
                println!("{:<28} max: {:<14} min: {}", it.name, f(&it.max), f(&it.min));
            }
        }
        Cmd::Settings { op } => {
            let mut lg = open_app(&cli)?;
            let mut s = lg.settings()?;
            match op {
                GetSet::Get => {
                    if cli.json {
                        return print_json(&s);
                    }
                    print_settings(&s);
                }
                GetSet::Set { values } => {
                    let kv = parse_kv(values)?;
                    apply_settings(&mut s, &kv)?;
                    lg.set_settings(&s)?;
                    println!("Save Success");
                }
            }
        }
        Cmd::Timezones => {
            for (i, (off, dst, name)) in TIMEZONES.iter().enumerate() {
                let d = if *dst { "  (DST)" } else { "" };
                println!("{i:3}  {name}{d}  [{off:+} min]");
            }
        }
        Cmd::Time { op } => {
            let mut lg = open_app(&cli)?;
            match op {
                TimeOp::Sync => lg.sync_time_utc()?,
                TimeOp::SyncNet => lg.sync_net_time()?,
            }
            println!("Sync Time Success");
        }
        Cmd::Calibration { op } => {
            let mut lg = open_app(&cli)?;
            let mut c = lg.calibration()?;
            match op {
                GetSet::Get => {
                    if cli.json {
                        return print_json(&c);
                    }
                    print_calibration(&c);
                }
                GetSet::Set { values } => {
                    apply_calibration(&mut c, &parse_kv(values)?)?;
                    lg.set_calibration(&c)?;
                    println!("Save Success");
                }
            }
        }
        Cmd::Alarms { op } => {
            let mut lg = open_app(&cli)?;
            let mut a = lg.alarms()?;
            match op {
                GetSet::Get => {
                    if cli.json {
                        return print_json(&a);
                    }
                    print_alarms(&a);
                }
                GetSet::Set { values } => {
                    apply_alarms(&mut a, &parse_kv(values)?)?;
                    lg.set_alarms(&a)?;
                    println!("Save Success");
                }
            }
        }
        Cmd::Rain { values } => {
            let kv = parse_kv(values)?;
            ensure!(!kv.is_empty(), "give at least one of day= week= month= year= total= (mm)");
            let get = |k: &str, max: u32| -> Result<Option<u32>> {
                kv.get(k)
                    .map(|v| {
                        let r = tenths(v)?;
                        ensure!((0..=max as i64).contains(&r), "{k} out of range");
                        Ok(r as u32)
                    })
                    .transpose()
            };
            for k in kv.keys() {
                ensure!(["day", "week", "month", "year", "total"].contains(&k.as_str()), "unknown key {k}");
            }
            let p = encode_rain(
                get("day", 9999)?.map(|v| v as u16),
                get("week", 99999)?,
                get("month", 99999)?,
                get("year", 99999)?,
                get("total", 99999)?,
            );
            open_app(&cli)?.set_rain(p)?;
            println!("Save Success");
        }
        Cmd::Mobile { op } => {
            let mut lg = open_app(&cli)?;
            match op {
                MobileOp::Get => {
                    let m = lg.mobile()?;
                    if cli.json {
                        return print_json(&m);
                    }
                    print_mobile(&m);
                }
                MobileOp::Set { values } => {
                    let mut m = lg.mobile()?;
                    apply_mobile(&mut m, &parse_kv(values)?)?;
                    lg.set_mobile(&m)?;
                    println!("Save Success");
                }
                MobileOp::TestSms => {
                    lg.test_sms()?;
                    println!("Test message requested");
                }
            }
        }
        Cmd::Upload { op } => {
            let mut lg = open_app(&cli)?;
            match op {
                UploadOp::Get { server } => {
                    let subs: Vec<u8> = match server {
                        Some(s) => vec![server_index(s)?],
                        None => (0..4).collect(),
                    };
                    let mut all = Vec::new();
                    for sub in subs {
                        all.push(lg.upload_server(sub)?);
                    }
                    if cli.json {
                        return print_json(&all);
                    }
                    for u in &all {
                        print_upload(u);
                    }
                }
                UploadOp::Set { server, values } => {
                    let sub = server_index(server)?;
                    let mut u = lg.upload_server(sub)?;
                    apply_upload(&mut u, &parse_kv(values)?)?;
                    lg.set_upload_server(&u)?;
                    println!("Save Success");
                }
            }
        }
        Cmd::Sensors { op } => {
            let mut lg = open_app(&cli)?;
            match op {
                SensorOp::Get => {
                    let mut all = Vec::new();
                    for (slot, _) in SENSOR_SLOTS {
                        all.push(lg.sensor(*slot)?);
                    }
                    if cli.json {
                        return print_json(&all);
                    }
                    for (r, (_, name)) in all.iter().zip(SENSOR_SLOTS) {
                        println!(
                            "slot {:>2}  {:<30} ID: {:<10} signal: {}  battery: {}",
                            r.slot,
                            name,
                            r.id_text(),
                            r.signal,
                            r.battery
                        );
                    }
                }
                SensorOp::Set { slot, id } => {
                    ensure!(
                        SENSOR_SLOTS.iter().any(|(s, _)| s == slot),
                        "unknown slot {slot}; valid: 0 2 3 6 14 22 31 40"
                    );
                    let id = if id.eq_ignore_ascii_case("learn") {
                        SENSOR_LEARNING
                    } else {
                        let v = u32::from_str_radix(id.trim_start_matches("0x"), 16)
                            .context("ID must be hex (max 8 digits) or `learn`")?;
                        ensure!(v < SENSOR_DISABLED, "reserved ID value");
                        v
                    };
                    lg.set_sensor_id(*slot, id)?;
                    println!("Save Success");
                }
            }
        }
        Cmd::Sd { op } => {
            let mut lg = open_app(&cli)?;
            match op {
                SdOp::List => {
                    let (files, sd) = lg.sd_list()?;
                    if cli.json {
                        return print_json(&files);
                    }
                    if let Some(s) = sd {
                        println!("SD card status: {}", sd_status_name(s));
                    }
                    for f in &files {
                        println!("{:<32} {}", f.name, human_size(f.size));
                    }
                }
                SdOp::Download { names, all, out, resume } => {
                    let (files, _) = lg.sd_list()?;
                    let wanted: Vec<&SdFile> = if *all {
                        files.iter().collect()
                    } else {
                        ensure!(!names.is_empty(), "name the files to download, or use --all");
                        names
                            .iter()
                            .map(|n| {
                                files
                                    .iter()
                                    .find(|f| &f.name == n)
                                    .ok_or_else(|| anyhow!("no file named {n:?} on the SD card"))
                            })
                            .collect::<Result<_>>()?
                    };
                    std::fs::create_dir_all(out)?;
                    for f in wanted {
                        let dest = safe_join(out, &f.name)?;
                        let n = lg.sd_download(f, &dest, *resume, |got, total| {
                            eprint!("\r{}: {got}/{total} bytes", f.name);
                        })?;
                        eprintln!("\r{}: {n} bytes -> {}", f.name, dest.display());
                    }
                }
                SdOp::Delete { name, yes } => {
                    confirm(&format!("Delete {name} from the SD card?"), *yes)?;
                    lg.sd_delete(name)?;
                    println!("Deleted {name}");
                }
            }
        }
        Cmd::Reset { factory, yes } => {
            let mut lg = open_app(&cli)?;
            if *factory {
                confirm("Reset the logger to factory settings?", *yes)?;
                lg.factory_reset()?;
            } else {
                confirm("Reboot the logger?", *yes)?;
                lg.reset_device()?;
            }
            println!("Success");
        }
        Cmd::Firmware { op } => match op {
            FwOp::Version => {
                let v = open(&cli)?.version()?;
                println!("{} : {}", v.label, v.version);
            }
            FwOp::Convert { hex, out } => {
                let img = load_hex(hex)?;
                std::fs::write(out, &img.data)?;
                println!("wrote {} ({} bytes)", out.display(), img.data.len());
            }
            FwOp::Update { hex, yes } => {
                let img = load_hex(hex)?;
                let mut lg = open(&cli)?;
                let v = lg.version()?;
                println!("Device: {} : {}", v.label, v.version);
                confirm(
                    "Flash this firmware? Do not unplug the logger until it finishes",
                    *yes,
                )?;
                let path = cli.device.clone();
                let mut reopen = || HidDevice::open_logger(path.as_deref());
                let mut last = String::new();
                firmware::update(&mut lg, &img, &mut reopen, &mut |stage, i, n| {
                    let pct = i * 100 / n;
                    let s = format!("{stage} {pct}%");
                    if s != last {
                        eprint!("\r{s}   ");
                        last = s;
                    }
                })?;
                eprintln!();
                println!("OK");
            }
        },
        Cmd::CheckUpdates => check_updates(&cli)?,
        Cmd::Raw { bytes, count, wait } => {
            let mut buf = Vec::new();
            for b in bytes.iter().flat_map(|s| s.split([',', ' '])) {
                if b.is_empty() {
                    continue;
                }
                buf.push(u8::from_str_radix(b.trim_start_matches("0x"), 16).with_context(|| format!("bad hex byte {b:?}"))?);
            }
            ensure!(!buf.is_empty() && buf.len() <= 64, "give 1..64 hex bytes");
            let mut lg = open(&cli)?;
            let replies = lg.raw(&buf, Duration::from_millis(*wait), *count)?;
            if replies.is_empty() {
                println!("(no reply)");
            }
            for r in replies {
                println!("{}", r.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ helpers

fn load_hex(p: &Path) -> Result<firmware::Image> {
    let text = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
    let img = firmware::parse_hex(&text)?;
    ensure!(img.inside > 0, "the HEX file has no data in the application area (0x3000..0x2A7F8)");
    if img.outside > 0 {
        eprintln!(
            "note: {} data bytes lie outside the application area and will not be flashed (same as the original app)",
            img.outside
        );
    }
    Ok(img)
}

/// Refuse path traversal in device-supplied file names.
fn safe_join(dir: &Path, name: &str) -> Result<PathBuf> {
    ensure!(
        !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != "..",
        "refusing unsafe file name {name:?}"
    );
    Ok(dir.join(name))
}

fn human_size(n: u32) -> String {
    let kb = n as f64 / 1024.0;
    if n <= 0x100000 {
        format!("{kb:.2} KB")
    } else {
        format!("{:.2} MB", kb / 1024.0)
    }
}

fn parse_kv(values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut m = BTreeMap::new();
    for v in values {
        let (k, val) = v
            .split_once('=')
            .ok_or_else(|| anyhow!("expected KEY=VALUE, got {v:?}"))?;
        m.insert(k.trim().to_ascii_lowercase().replace('-', "_"), val.trim().to_string());
    }
    Ok(m)
}

fn parse_bool(s: &str) -> Result<bool> {
    match s.to_ascii_lowercase().as_str() {
        "1" | "on" | "yes" | "true" | "enable" | "enabled" => Ok(true),
        "0" | "off" | "no" | "false" | "disable" | "disabled" => Ok(false),
        _ => bail!("expected on/off, got {s:?}"),
    }
}

/// Decimal string -> tenths, rounded (the app's validators round).
fn tenths(s: &str) -> Result<i64> {
    let v: f64 = s.parse().with_context(|| format!("not a number: {s:?}"))?;
    Ok((v * 10.0).round() as i64)
}

fn is_unset(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "none" | "unset" | "--")
}

fn unknown_keys(kv: &BTreeMap<String, String>, known: &[&str]) -> Result<()> {
    for k in kv.keys() {
        ensure!(known.contains(&k.as_str()), "unknown key {k:?}; valid keys: {}", known.join(", "));
    }
    Ok(())
}

fn server_index(s: &str) -> Result<u8> {
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

// ---------------------------------------------------------------- printing

fn print_live(l: &Live, u: &Units) {
    use chrono::DateTime;
    let t = DateTime::from_timestamp(l.device_time as i64, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "?".into());
    let net = match l.network {
        1 => "2G",
        2 => "3G",
        3 => "4G",
        4 => "5G",
        _ => "-",
    };
    println!("Time             {t}");
    if l.learning_countdown_s > 0 {
        println!("Learning sensors ... {} s", l.learning_countdown_s);
    }
    if l.sim_present {
        println!("Mobile           {net}  signal {}/5", l.signal);
    } else {
        println!("Mobile           NO SIMCARD");
    }
    if l.battery_pct <= 100 {
        println!("Battery          {} %", l.battery_pct);
    }
    let rows: Vec<(&str, String)> = vec![
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
    ];
    for (k, v) in rows {
        println!("{k:<16} {v}");
    }
}

fn print_status(cli: &Cli, s: &Status) -> Result<()> {
    if cli.json {
        return print_json(s);
    }
    println!("GSM state        {}", s.gsm_state_name());
    println!("SD card          {}", s.sd_status_name());
    println!("Battery voltage  {:.4} V", s.battery_volts);
    println!("Current task     0x{:08X}   wait task 0x{:08X}", s.current_task, s.wait_task);
    println!("GSM restarts     auto {}  error {}", s.gsm_auto_restarts, s.gsm_error_restarts);
    println!(
        "Uploads OK       WU {}  WCLD {}  WOW {}  user {}",
        s.upload_ok_wu, s.upload_ok_wcld, s.upload_ok_wow, s.upload_ok_user
    );
    println!(
        "Connect fails    WU {}  WCLD {}  WOW {}  user {}  time {}",
        s.connect_fail_wu, s.connect_fail_wcld, s.connect_fail_wow, s.connect_fail_user, s.connect_fail_time
    );
    Ok(())
}

fn fmt_mm(v: &MmValue, u: &Units) -> String {
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

fn name_of(names: &[&str], i: u8) -> String {
    names.get(i as usize).map(|s| s.to_string()).unwrap_or_else(|| format!("?{i}"))
}

fn print_settings(s: &Settings) {
    let u = &s.units;
    let sensor = |v: u8, n: &str| if v == 0 { n.to_string() } else { "none".to_string() };
    println!("temp_unit={}", name_of(units::TEMP_NAMES, u.temp));
    println!("pressure_unit={}", name_of(units::PRESSURE_NAMES, u.pressure));
    println!("wind_unit={}", name_of(units::WIND_NAMES, u.wind));
    println!("rain_unit={}", name_of(units::RAIN_NAMES, u.rain));
    println!("light_unit={}", name_of(units::LIGHT_NAMES, u.light));
    println!("timezone={}    # {}", s.timezone, timezone_name(s.timezone));
    println!("dst={}", if s.dst { "on" } else { "off" });
    println!("interval={}    # minutes", s.interval_s / 60);
    let outdoor = OUTDOOR_SENSORS
        .iter()
        .find(|(v, _)| *v == s.outdoor_sensor)
        .map(|x| x.1)
        .unwrap_or("?");
    println!("outdoor_sensor={outdoor}");
    println!("indoor_sensor={}", sensor(s.indoor_sensor, "wh30"));
    println!("soil_sensor={}", sensor(s.soil_sensor, "wh51"));
    println!("pm25_sensor={}", sensor(s.pm25_sensor, "wh41"));
    println!("area={}", name_of(AREAS, s.area));
    println!("frequency={}    # read-only", name_of(FREQUENCIES, s.frequency));
    println!("name={}", s.name);
}

fn apply_settings(s: &mut Settings, kv: &BTreeMap<String, String>) -> Result<()> {
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

fn print_calibration(c: &Calibration) {
    println!("in_temp={:.1}    # °C offset", c.in_temp as f64 / 10.0);
    println!("out_temp={:.1}", c.out_temp as f64 / 10.0);
    println!("in_humi={}    # % offset", c.in_humi);
    println!("out_humi={}", c.out_humi);
    println!("abs_pressure={:.1}    # hPa offset", c.abs_pressure as f64 / 10.0);
    println!("rel_pressure={:.1}", c.rel_pressure as f64 / 10.0);
    println!("wind_dir={}    # degrees", c.wind_dir);
    println!("wind_gain={:.2}", c.wind_gain as f64 / 100.0);
    println!("rain_gain={:.2}", c.rain_gain as f64 / 100.0);
    println!("light_gain={:.2}", c.light_gain as f64 / 100.0);
}

fn apply_calibration(c: &mut Calibration, kv: &BTreeMap<String, String>) -> Result<()> {
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

fn fmt_t16(v: i16) -> String {
    if v == 0x7FFF { "none".into() } else { format!("{:.1}", v as f64 / 10.0) }
}
fn fmt_u16(v: u16) -> String {
    if v == 0xFFFF { "none".into() } else { format!("{:.1}", v as f64 / 10.0) }
}
fn fmt_u8(v: u8) -> String {
    if v == 0xFF { "none".into() } else { v.to_string() }
}
fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn print_alarms(a: &Alarms) {
    for (k, v, unit) in [("in_temp", &a.in_temp, "°C"), ("out_temp", &a.out_temp, "°C"), ("dew_point", &a.dew_point, "°C")] {
        println!("{k}_high={}  {k}_high_on={}  {k}_low={}  {k}_low_on={}    # {unit}",
            fmt_t16(v.high.value), on(v.high.enabled), fmt_t16(v.low.value), on(v.low.enabled));
    }
    for (k, v) in [("in_humi", &a.in_humi), ("out_humi", &a.out_humi)] {
        println!("{k}_high={}  {k}_high_on={}  {k}_low={}  {k}_low_on={}    # %",
            fmt_u8(v.high.value), on(v.high.enabled), fmt_u8(v.low.value), on(v.low.enabled));
    }
    for (k, v, unit) in [("wind", &a.wind, "m/s"), ("gust", &a.gust, "m/s"), ("rain_rate", &a.rain_rate, "mm/h"), ("rain_day", &a.rain_day, "mm")] {
        println!("{k}={}  {k}_on={}    # {unit}", fmt_u16(v.value), on(v.enabled));
    }
    match a.master {
        Some(m) => println!("master={}", on(m)),
        None => println!("# master alarm switch not supported by this firmware"),
    }
}

fn apply_alarms(a: &mut Alarms, kv: &BTreeMap<String, String>) -> Result<()> {
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

fn fmt_interval(s: u32) -> String {
    if s == 0xFFFF_FFFF {
        "none".into()
    } else {
        (s / 60).to_string()
    }
}

fn print_mobile(m: &Mobile) {
    println!("apn={}", m.apn);
    println!("user={}", m.user);
    println!("password={}", m.password);
    println!("sms={}", on(m.sms_enabled));
    let alarm = if m.sms_alarm_interval_s == 0xFFFF { "none".into() } else { (m.sms_alarm_interval_s / 60).to_string() };
    println!("alarm_interval_min={alarm}");
    println!("send_interval_min={}", fmt_interval(m.sms_send_interval_s));
    for (i, p) in m.phones.iter().enumerate() {
        println!("phone{}={p}", i + 1);
    }
}

fn apply_mobile(m: &mut Mobile, kv: &BTreeMap<String, String>) -> Result<()> {
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

fn print_upload(u: &UploadServer) {
    println!("[{}]", SERVERS[u.sub as usize]);
    println!("enabled={}", on(u.enabled()));
    println!("host={}", u.host);
    println!("port={}", u.port);
    println!("interval_min={}", fmt_interval(u.interval_s));
    println!("station_id={}", u.station_id);
    println!("password={}", u.password);
    println!("type={}", name_of(SERVER_TYPES, u.server_type));
    println!();
}

fn apply_upload(u: &mut UploadServer, kv: &BTreeMap<String, String>) -> Result<()> {
    unknown_keys(kv, &["enabled", "host", "port", "interval_min", "station_id", "password", "type"])?;
    for (k, v) in kv {
        match k.as_str() {
            "enabled" => u.flags = (u.flags & !1) | parse_bool(v)? as u8,
            "host" => {
                ensure!(u.sub == 3, "only the custom server's host can be changed");
                u.host = v.clone();
            }
            "port" => u.port = v.parse().context("port")?,
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

// ----------------------------------------------------------- online check

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

fn check_updates(cli: &Cli) -> Result<()> {
    let fw = fetch_ini("http://download.ecowitt.net/down/filewave?v=FirwaveReadme.txt")?;
    let ver = ini_get(&fw, "WH6006", "VER").ok_or_else(|| anyhow!("no [WH6006] VER in firmware readme"))?;
    let url = ini_get(&fw, "WH6006", "ULR").unwrap_or_default();
    let notes = ini_get(&fw, "WH6006", "NOTES").unwrap_or_default().replace(';', "\n  ");
    println!("Latest firmware: {ver}");
    println!("  download: {url}");
    if !notes.is_empty() {
        println!("  {notes}");
    }
    match open(cli).and_then(|mut lg| lg.version()) {
        Ok(v) if !v.is_bootloader() => {
            let newer = Version::numeric(&ver) > Version::numeric(&v.version);
            println!(
                "Device firmware: {}{}",
                v.version,
                if newer { "  -> update available (download the .hex and run `firmware update`)" } else { "  (up to date)" }
            );
        }
        Ok(_) => println!("Device is in bootloader mode"),
        Err(_) => println!("(no device connected)"),
    }
    Ok(())
}
