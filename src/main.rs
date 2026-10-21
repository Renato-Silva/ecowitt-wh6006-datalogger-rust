//! wh6006 — Linux command-line replacement for Ecowitt's "Weather Logger"
//! Windows app (v1.7.3) for the WH6006 GSM weather logger.

use anyhow::{anyhow, ensure, Context, Result};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use wh6006::device::Logger;
use wh6006::form::{self, parse_kv, Row};
use wh6006::hid::{self, HidDevice};
use wh6006::model::*;
use wh6006::units::Units;
use wh6006::view::{self, human_size, safe_join};
use wh6006::firmware;

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
    /// Print the device log (modem AT commands and replies) as it arrives
    Log {
        /// Stop after N seconds (default: until Ctrl-C)
        #[arg(long, value_name = "N")]
        seconds: Option<u64>,
    },
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
    /// KEY=VALUE: host port station_id password type=jsp|asp|php, plus
    /// enabled=on|off and interval_min, which apply to all servers.
    /// Like the Windows app, this rewrites all four servers.
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
                let f = |v: &Option<MmValue>| v.as_ref().map(|v| view::fmt_mm(v, &u)).unwrap_or_default();
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
                    print_rows(&form::settings_rows(&s));
                }
                GetSet::Set { values } => {
                    form::apply_settings(&mut s, &parse_kv(values)?)?;
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
                    print_rows(&form::calibration_rows(&c));
                }
                GetSet::Set { values } => {
                    form::apply_calibration(&mut c, &parse_kv(values)?)?;
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
                    print_rows(&form::alarms_rows(&a));
                }
                GetSet::Set { values } => {
                    form::apply_alarms(&mut a, &parse_kv(values)?)?;
                    lg.set_alarms(&a)?;
                    println!("Save Success");
                }
            }
        }
        Cmd::Rain { values } => {
            let p = form::rain_packet(&parse_kv(values)?)?;
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
                    print_rows(&form::mobile_rows(&m));
                }
                MobileOp::Set { values } => {
                    let mut m = lg.mobile()?;
                    form::apply_mobile(&mut m, &parse_kv(values)?)?;
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
                    let all = match server {
                        Some(s) => vec![lg.upload_server(form::server_index(s)?)?],
                        None => lg.upload_servers()?,
                    };
                    if cli.json {
                        return print_json(&all);
                    }
                    for u in &all {
                        println!("[{}]", SERVERS[u.sub as usize]);
                        print_rows(&form::upload_server_rows(u));
                        println!();
                    }
                }
                UploadOp::Set { server, values } => {
                    let name = SERVERS[form::server_index(server)? as usize];
                    let kv = parse_kv(values)?
                        .into_iter()
                        .map(|(k, v)| if form::UPLOAD_SHARED.contains(&k.as_str()) { (k, v) } else { (format!("{name}.{k}"), v) })
                        .collect();
                    let mut all = lg.upload_servers()?;
                    form::apply_uploads(&mut all, &kv)?;
                    lg.set_upload_servers(&all)?;
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
                    let id = form::parse_sensor_id(*slot, id)?;
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
        Cmd::Log { seconds } => {
            let mut lg = open(&cli)?;
            let end = seconds.map(|s| Instant::now() + Duration::from_secs(s));
            eprintln!("waiting for device log text (reboot the logger to see its start-up; Ctrl-C to stop)");
            let mut out = std::io::stdout();
            while end.is_none_or(|e| Instant::now() < e) {
                // Replies to other packets are of no interest here.
                let _ = lg.c.recv(Duration::from_millis(500))?;
                for chunk in lg.c.take_log() {
                    write!(out, "{}", chunk.replace('\r', ""))?;
                    out.flush()?;
                }
            }
        }
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
    let img = firmware::load_hex(p)?;
    if img.outside > 0 {
        eprintln!(
            "note: {} data bytes lie outside the application area and will not be flashed (same as the original app)",
            img.outside
        );
    }
    Ok(img)
}

// ---------------------------------------------------------------- printing

fn print_rows(rows: &[Row]) {
    for r in rows {
        println!("{}", r.to_line());
    }
}

fn print_live(l: &Live, u: &Units) {
    println!("Time             {}", view::device_time(l));
    if l.learning_countdown_s > 0 {
        println!("Learning sensors ... {} s", l.learning_countdown_s);
    }
    if l.sim_present {
        println!("Mobile           {}  signal {}/5", view::network_name(l), l.signal);
    } else {
        println!("Mobile           NO SIMCARD");
    }
    if l.battery_pct <= 100 {
        println!("Battery          {} %", l.battery_pct);
    }
    for (k, v) in view::live_rows(l, u) {
        println!("{k:<16} {v}");
    }
}

fn print_status(cli: &Cli, s: &Status) -> Result<()> {
    if cli.json {
        return print_json(s);
    }
    for (k, v) in view::status_rows(s) {
        println!("{k:<16} {v}");
    }
    Ok(())
}

// ----------------------------------------------------------- online check

fn check_updates(cli: &Cli) -> Result<()> {
    let fw = view::latest_firmware()?;
    println!("Latest firmware: {}", fw.version);
    println!("  download: {}", fw.url);
    if !fw.notes.is_empty() {
        println!("  {}", fw.notes.replace('\n', "\n  "));
    }
    match open(cli).and_then(|mut lg| lg.version()) {
        Ok(v) if !v.is_bootloader() => {
            let newer = Version::numeric(&fw.version) > Version::numeric(&v.version);
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
