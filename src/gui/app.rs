//! The window: a tab per area of the original Windows app.

use crate::worker::{Cmd, Reply, Section, Worker};
use eframe::egui::{self, Color32, RichText};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use wh6006::form::{Kind, Kv, Row, RAIN_KEYS};
use wh6006::model::*;
use wh6006::units::Units;
use wh6006::view::{self, LatestFirmware};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Live,
    MaxMin,
    Settings,
    Calibration,
    Alarms,
    Mobile,
    Upload,
    Sensors,
    Sd,
    Device,
    Log,
}

const TABS: &[(Tab, &str)] = &[
    (Tab::Live, "Live data"),
    (Tab::MaxMin, "Max / Min"),
    (Tab::Settings, "Settings"),
    (Tab::Calibration, "Calibration"),
    (Tab::Alarms, "Alarms"),
    (Tab::Mobile, "Mobile"),
    (Tab::Upload, "Upload"),
    (Tab::Sensors, "Sensors"),
    (Tab::Sd, "SD card"),
    (Tab::Device, "Device"),
    (Tab::Log, "Device log"),
];

const LOG_MAX_LINES: usize = 5000;

/// Editable copy of a key=value block.
#[derive(Default)]
struct Form {
    rows: Vec<Row>,
    edits: Vec<Vec<String>>,
}

impl Form {
    fn set(&mut self, rows: Vec<Row>) {
        self.edits = rows.iter().map(|r| r.fields.iter().map(|f| f.value.clone()).collect()).collect();
        self.rows = rows;
    }

    fn revert(&mut self) {
        let rows = std::mem::take(&mut self.rows);
        self.set(rows);
    }

    fn changes(&self) -> Kv {
        let mut kv = BTreeMap::new();
        for (r, e) in self.rows.iter().zip(&self.edits) {
            for (f, v) in r.fields.iter().zip(e) {
                if f.value != *v {
                    kv.insert(f.key.clone(), v.trim().to_string());
                }
            }
        }
        kv
    }
}

struct Confirm {
    title: String,
    text: String,
    action: String,
    cmd: Cmd,
}

struct Message {
    text: String,
    error: bool,
    at: Instant,
}

pub struct App {
    tx: Sender<Cmd>,
    rx: Receiver<Reply>,
    pending: usize,
    live_pending: bool,

    tab: Tab,
    connected: bool,
    version: Option<Version>,
    name: String,
    device_units: Units,
    metric: bool,
    message: Option<Message>,
    progress: Option<(String, f32)>,
    confirm: Option<Confirm>,

    live: Option<Live>,
    live_at: Option<Instant>,
    auto_refresh: bool,
    refresh_secs: u32,
    hide_missing: bool,

    maxmin: Vec<MaxMinItem>,
    forms: HashMap<Section, Form>,
    rain: BTreeMap<&'static str, String>,
    status: Option<Status>,
    sensors: Vec<SensorRecord>,
    sensor_edit: HashMap<u8, String>,
    sd_files: Vec<SdFile>,
    sd_status: Option<u8>,
    sd_selected: Vec<bool>,
    sd_dir: String,
    latest: Option<(LatestFirmware, Option<Version>)>,
    hex_path: String,
    log: Vec<String>,
    log_partial: String,
    log_errors_only: bool,
    /// Last SIM state the modem reported in the log.
    sim_missing: Option<bool>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, device: Option<PathBuf>, debug: bool) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let worker = Worker::new(device, debug, reply_tx, cc.egui_ctx.clone());
        std::thread::spawn(move || worker.run(cmd_rx));
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let mut app = Self {
            tx: cmd_tx,
            rx: reply_rx,
            pending: 0,
            live_pending: false,
            tab: Tab::Live,
            connected: false,
            version: None,
            name: String::new(),
            device_units: Units::METRIC,
            metric: false,
            message: None,
            progress: None,
            confirm: None,
            live: None,
            live_at: None,
            auto_refresh: true,
            refresh_secs: 10,
            hide_missing: true,
            maxmin: Vec::new(),
            forms: HashMap::new(),
            rain: RAIN_KEYS.iter().map(|k| (*k, String::new())).collect(),
            status: None,
            sensors: Vec::new(),
            sensor_edit: HashMap::new(),
            sd_files: Vec::new(),
            sd_status: None,
            sd_selected: Vec::new(),
            sd_dir: format!("{home}/wh6006"),
            latest: None,
            hex_path: String::new(),
            log: Vec::new(),
            log_partial: String::new(),
            log_errors_only: false,
            sim_missing: None,
        };
        app.send(Cmd::Connect);
        app
    }

    fn send(&mut self, cmd: Cmd) {
        if matches!(cmd, Cmd::Live) {
            self.live_pending = true;
        }
        self.pending += 1;
        let _ = self.tx.send(cmd);
    }

    fn ask(&mut self, title: &str, text: &str, action: &str, cmd: Cmd) {
        self.confirm = Some(Confirm { title: title.into(), text: text.into(), action: action.into(), cmd });
    }

    fn units(&self) -> Units {
        if self.metric { Units::METRIC } else { self.device_units }
    }

    fn in_bootloader(&self) -> bool {
        self.version.as_ref().is_some_and(|v| v.is_bootloader())
    }

    /// Load whatever the current tab shows.
    fn refresh_tab(&mut self) {
        if !self.connected || self.in_bootloader() {
            return;
        }
        match self.tab {
            Tab::Live => {
                if !self.live_pending {
                    self.send(Cmd::Live);
                }
            }
            Tab::MaxMin => self.send(Cmd::MaxMin),
            Tab::Settings => self.send(Cmd::Load(Section::Settings)),
            Tab::Calibration => self.send(Cmd::Load(Section::Calibration)),
            Tab::Alarms => self.send(Cmd::Load(Section::Alarms)),
            Tab::Mobile => self.send(Cmd::Load(Section::Mobile)),
            Tab::Upload => self.send(Cmd::Load(Section::Upload)),
            Tab::Sensors => self.send(Cmd::Sensors),
            Tab::Sd => self.send(Cmd::SdList),
            Tab::Device => self.send(Cmd::Info),
            Tab::Log => {}
        }
    }

    fn add_log(&mut self, chunks: &[String]) {
        for c in chunks {
            self.log_partial.push_str(&c.replace('\r', ""));
        }
        while let Some(i) = self.log_partial.find('\n') {
            let line: String = self.log_partial.drain(..=i).collect();
            let line = line.trim_end().to_string();
            if line.is_empty() {
                continue;
            }
            if line.contains("SIM not inserted") || line.contains("NOT INSERTED") {
                self.sim_missing = Some(true);
            } else if line.contains("+CPIN: READY") {
                self.sim_missing = Some(false);
            }
            self.log.push(line);
        }
        if self.log.len() > LOG_MAX_LINES {
            self.log.drain(..self.log.len() - LOG_MAX_LINES);
        }
    }

    fn set_message(&mut self, text: String, error: bool) {
        self.message = Some(Message { text, error, at: Instant::now() });
    }

    fn poll(&mut self) {
        while let Ok(r) = self.rx.try_recv() {
            match r {
                Reply::Connected { version, name, units } => {
                    let first = !self.connected;
                    self.connected = true;
                    self.name = name;
                    self.device_units = units;
                    self.version = Some(version);
                    if self.in_bootloader() {
                        self.tab = Tab::Device;
                    } else if first {
                        self.refresh_tab();
                    }
                }
                Reply::Disconnected => {
                    self.connected = false;
                    self.live_pending = false;
                }
                Reply::Info { version, name, status } => {
                    self.version = Some(version);
                    self.name = name;
                    self.status = Some(status);
                }
                Reply::Live(l) => {
                    self.live = Some(l);
                    self.live_at = Some(Instant::now());
                }
                Reply::Units(u) => self.device_units = u,
                Reply::MaxMin(m) => self.maxmin = m,
                Reply::Rows(section, rows) => {
                    if section == Section::Settings
                        && let Some(n) = rows.iter().flat_map(|r| &r.fields).find(|f| f.key == "name")
                    {
                        self.name = n.value.clone();
                    }
                    self.forms.entry(section).or_default().set(rows);
                }
                Reply::Sensors(s) => {
                    self.sensors = s;
                    self.sensor_edit.clear();
                }
                Reply::Sd(files, status) => {
                    self.sd_selected = vec![false; files.len()];
                    self.sd_files = files;
                    self.sd_status = status;
                }
                Reply::Updates { latest, device } => self.latest = Some((latest, device)),
                Reply::Progress(label, f) => self.progress = Some((label, f)),
                Reply::Log(chunks) => self.add_log(&chunks),
                Reply::Ok(text) => {
                    self.progress = None;
                    self.set_message(text, false);
                }
                Reply::Err(text) => {
                    self.progress = None;
                    self.set_message(text, true);
                }
                Reply::Done => {
                    self.pending = self.pending.saturating_sub(1);
                    if self.pending == 0 {
                        self.live_pending = false;
                        self.progress = None;
                    }
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();

        // Live auto-refresh.
        if self.tab == Tab::Live && self.auto_refresh && self.connected && !self.in_bootloader() {
            let every = Duration::from_secs(self.refresh_secs.max(2) as u64);
            let due = self.live_at.is_none_or(|t| t.elapsed() >= every);
            if due && !self.live_pending && self.pending == 0 {
                self.send(Cmd::Live);
            }
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }

        self.top_bar(ui);
        self.status_bar(ui);
        self.nav(ui);
        egui::CentralPanel::default().show(ui, |ui| {
            // The log has its own scroll area that sticks to the bottom.
            if self.tab == Tab::Log {
                self.log_ui(ui);
                return;
            }
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                if !self.connected {
                    self.disconnected_ui(ui);
                    return;
                }
                match self.tab {
                    Tab::Live => self.live_ui(ui),
                    Tab::MaxMin => self.maxmin_ui(ui),
                    Tab::Settings => self.settings_ui(ui),
                    Tab::Calibration => self.calibration_ui(ui),
                    Tab::Alarms => self.alarms_ui(ui),
                    Tab::Mobile => self.mobile_ui(ui),
                    Tab::Upload => self.upload_ui(ui),
                    Tab::Sensors => self.sensors_ui(ui),
                    Tab::Sd => self.sd_ui(ui),
                    Tab::Device => self.device_ui(ui),
                    Tab::Log => {}
                }
            });
        });
        self.confirm_ui(ui.ctx());
    }
}

// ------------------------------------------------------------------ chrome

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("WH6006 Weather Logger");
                ui.separator();
                if self.connected {
                    let v = self.version.as_ref().map(|v| format!("{} {}", v.label, v.version)).unwrap_or_default();
                    dot(ui, Color32::from_rgb(60, 180, 90));
                    if !self.name.is_empty() {
                        ui.strong(&self.name);
                    }
                    ui.weak(v);
                } else {
                    dot(ui, Color32::GRAY);
                    ui.weak("Not connected");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.connected { "Reconnect" } else { "Connect" };
                    if ui.add_enabled(self.pending == 0, egui::Button::new(label)).clicked() {
                        self.send(Cmd::Connect);
                    }
                    if self.pending > 0 {
                        ui.spinner();
                    }
                });
            });
            ui.add_space(4.0);
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if let Some((label, f)) = &self.progress {
                    ui.add(egui::ProgressBar::new(*f).text(format!("{label}  {:.0}%", f * 100.0)).desired_width(320.0));
                } else if let Some(m) = &self.message {
                    let color = if m.error { ui.visuals().error_fg_color } else { ui.visuals().text_color() };
                    let ago = m.at.elapsed().as_secs();
                    ui.label(RichText::new(&m.text).color(color));
                    if ago >= 60 {
                        ui.weak(format!("({} min ago)", ago / 60));
                    }
                } else {
                    ui.weak("Ready");
                }
            });
            ui.add_space(2.0);
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("nav").resizable(false).exact_size(150.0).show(ui, |ui| {
            ui.add_space(8.0);
            let enabled = self.connected && !self.in_bootloader();
            for (tab, label) in TABS {
                let ok = enabled || matches!(tab, Tab::Device | Tab::Log);
                let r = ui.add_enabled(ok, egui::Button::selectable(self.tab == *tab, *label).min_size(egui::vec2(ui.available_width(), 26.0)));
                if r.clicked() && self.tab != *tab {
                    self.tab = *tab;
                    self.refresh_tab();
                }
            }
        });
    }

    fn disconnected_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.heading("No logger connected");
            ui.add_space(8.0);
            ui.label("Plug in the WH6006 over USB (it shows up as 04d8:003c) and press Connect.");
            ui.label("If it is plugged in but can't be opened, install the udev rule from the README:");
            ui.code("sudo cp 70-wh6006.rules /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger");
            ui.add_space(12.0);
            if ui.add_enabled(self.pending == 0, egui::Button::new("Connect")).clicked() {
                self.send(Cmd::Connect);
            }
        });
    }

    fn confirm_ui(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.confirm else { return };
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(&c.title);
            ui.add_space(6.0);
            ui.label(&c.text);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new(&c.action).color(ui.visuals().warn_fg_color)).clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        if let Some(yes) = answer {
            let c = self.confirm.take().expect("confirm shown");
            if yes {
                self.send(c.cmd);
            }
        }
    }
}

fn header(ui: &mut egui::Ui, title: &str, refresh: &mut bool) {
    ui.horizontal(|ui| {
        ui.heading(title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Reload").clicked() {
                *refresh = true;
            }
        });
    });
    ui.add_space(6.0);
}

// ------------------------------------------------------------------- forms

fn pretty(key: &str) -> String {
    // Upload keys are `<server>.<key>`.
    let key = key.rsplit_once('.').map_or(key, |(_, k)| k);
    let s = match key {
        "enabled" => "Upload",
        "type" => "Server type",
        "temp_unit" => "Temperature unit",
        "in_temp" => "Indoor temperature",
        "out_temp" => "Outdoor temperature",
        "in_humi" => "Indoor humidity",
        "out_humi" => "Outdoor humidity",
        "dew_point" => "Dew point",
        "abs_pressure" => "Absolute pressure",
        "rel_pressure" => "Relative pressure",
        "wind_dir" => "Wind direction",
        "pm25_sensor" => "PM2.5 sensor",
        "dst" => "Daylight saving",
        "interval" => "Logging interval",
        "name" => "Device name",
        "apn" => "APN",
        "user" => "APN user",
        "password" => "Password",
        "sms" => "SMS",
        "alarm_interval_min" => "Alarm SMS interval (min)",
        "send_interval_min" => "Report SMS interval (min)",
        "interval_min" => "Upload interval (min)",
        "station_id" => "Station ID",
        "master" => "Master alarm switch",
        "rain_rate" => "Rain rate",
        "rain_day" => "Rain per day",
        k => {
            let mut s = k.replace('_', " ");
            if let Some(c) = s.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            return s;
        }
    };
    s.to_string()
}

fn is_on_off(kind: &Kind) -> bool {
    matches!(kind, Kind::Choice(c) if c.len() == 2 && c[0].0 == "on" && c[1].0 == "off")
}

fn field_ui(ui: &mut egui::Ui, id: &str, f: &wh6006::form::Field, value: &mut String, label: &str) {
    let changed = f.value != *value;
    match &f.kind {
        k if is_on_off(k) => {
            let mut b = value == "on";
            let text = if changed { RichText::new(label).strong() } else { RichText::new(label) };
            if ui.checkbox(&mut b, text).on_hover_text(&f.key).changed() {
                *value = if b { "on" } else { "off" }.into();
            }
        }
        Kind::Choice(choices) => {
            let current = choices.iter().find(|(v, _)| v == value).map(|(_, l)| l.as_str()).unwrap_or(value);
            let text = if changed { RichText::new(current).strong() } else { RichText::new(current) };
            egui::ComboBox::from_id_salt((id, &f.key))
                .selected_text(text)
                .width(220.0)
                .height(360.0)
                .show_ui(ui, |ui| {
                    for (v, l) in choices {
                        ui.selectable_value(value, v.clone(), l);
                    }
                });
        }
        Kind::ReadOnly => {
            ui.label(value.as_str());
        }
        Kind::Text | Kind::Secret => {
            if !label.is_empty() {
                ui.label(label);
            }
            let mut te = egui::TextEdit::singleline(value)
                .desired_width(if label.is_empty() { 220.0 } else { 70.0 })
                .password(f.kind == Kind::Secret);
            if changed {
                te = te.text_color(ui.visuals().warn_fg_color);
            }
            ui.add(te).on_hover_text(&f.key);
        }
    }
}

/// Draws a form; returns `Some(changes)` when Save was pressed.
fn form_ui(ui: &mut egui::Ui, id: &str, form: &mut Form, busy: bool) -> Option<Kv> {
    if form.rows.is_empty() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Loading…");
        });
        return None;
    }
    egui::Grid::new(id).num_columns(3).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
        for (row, edits) in form.rows.iter().zip(form.edits.iter_mut()) {
            if row.fields.is_empty() && row.note.is_none() {
                ui.add_space(8.0);
                ui.end_row();
                ui.strong(&row.label);
                ui.end_row();
                continue;
            }
            if row.fields.is_empty() {
                ui.label(pretty(&row.label));
                ui.weak(row.note.as_deref().unwrap_or(""));
                ui.end_row();
                continue;
            }
            ui.label(pretty(&row.label));
            ui.horizontal(|ui| {
                let multi = row.fields.len() > 1;
                for (f, v) in row.fields.iter().zip(edits.iter_mut()) {
                    let label = if multi {
                        match f.key.strip_prefix(&row.label).unwrap_or(&f.key).trim_start_matches('_') {
                            "" => "",
                            "high" => "high",
                            "low" => "low",
                            _ if is_on_off(&f.kind) => "on",
                            s => s,
                        }
                    } else {
                        ""
                    };
                    field_ui(ui, id, f, v, label);
                    if multi && is_on_off(&f.kind) {
                        ui.add_space(12.0);
                    }
                }
            });
            match &row.note {
                Some(n) if row.label != "timezone" => ui.weak(n),
                _ => ui.label(""),
            };
            ui.end_row();
        }
    });
    ui.add_space(10.0);
    let changes = form.changes();
    let mut save = None;
    ui.horizontal(|ui| {
        let n = changes.len();
        let label = match n {
            0 => "Save".to_string(),
            1 => "Save 1 change".to_string(),
            n => format!("Save {n} changes"),
        };
        if ui.add_enabled(n > 0 && !busy, egui::Button::new(label)).clicked() {
            save = Some(changes.clone());
        }
        if ui.add_enabled(n > 0, egui::Button::new("Revert")).clicked() {
            form.revert();
        }
    });
    save
}

impl App {
    fn section_form(&mut self, ui: &mut egui::Ui, section: Section, id: &str) {
        let busy = self.pending > 0;
        let form = self.forms.entry(section).or_default();
        if let Some(kv) = form_ui(ui, id, form, busy) {
            self.send(Cmd::Save(section, kv));
        }
    }
}

// -------------------------------------------------------------------- tabs

impl App {
    fn live_ui(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        header(ui, "Live data", &mut refresh);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.auto_refresh, "Refresh every");
            ui.add(egui::DragValue::new(&mut self.refresh_secs).range(2..=600).suffix(" s"));
            ui.separator();
            ui.checkbox(&mut self.metric, "Metric units");
            ui.checkbox(&mut self.hide_missing, "Hide sensors without data");
        });
        ui.add_space(8.0);
        if refresh && !self.live_pending {
            self.send(Cmd::Live);
        }
        if self.live.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Reading…");
            });
            return;
        }
        let u = self.units();
        self.sim_banner(ui);
        let Some(l) = &self.live else { return };
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Logger time {}", view::device_time(l)));
            ui.separator();
            if l.sim_present {
                ui.label(format!("Mobile {}  signal {}/5", view::network_name(l), l.signal));
            } else {
                ui.label(RichText::new("No SIM card").color(ui.visuals().warn_fg_color));
            }
            if l.battery_pct <= 100 {
                ui.separator();
                // Firmware thresholds: below 5 % it stops uploads and SMS
                // until the battery is back at 60 %; below 10 % it warns.
                let text = format!("Battery {} %", l.battery_pct);
                match l.battery_pct {
                    0..5 => ui.label(RichText::new(text + " - uploads and SMS suspended").color(ui.visuals().error_fg_color)),
                    5..10 => ui.label(RichText::new(text + " - low").color(ui.visuals().warn_fg_color)),
                    _ => ui.label(text),
                };
            }
            if l.learning_countdown_s > 0 {
                ui.separator();
                ui.label(format!("Learning sensors… {} s", l.learning_countdown_s));
            }
            if let Some(t) = self.live_at {
                ui.separator();
                ui.weak(format!("updated {} s ago", t.elapsed().as_secs()));
            }
        });
        ui.add_space(10.0);

        let rows = view::live_rows(l, &u);
        let groups: &[(&str, &[&str])] = &[
            ("Temperature & humidity", &["Outdoor temp", "Outdoor humidity", "Indoor temp", "Indoor humidity", "Dew point", "Wind chill", "Heat index"]),
            ("Wind & pressure", &["Wind", "Gust", "Wind direction", "Rel pressure", "Abs pressure"]),
            ("Rain", &["Rain rate", "Rain event", "Rain hour", "Rain day", "Rain week", "Rain month", "Rain year", "Rain total"]),
            ("Light & extra sensors", &["Light", "UVI", "Soil moisture", "PM2.5", "Temperature", "Leaf wetness"]),
        ];
        for (title, names) in groups {
            let items: Vec<&(&str, String)> = names
                .iter()
                .filter_map(|n| rows.iter().find(|(k, _)| k == n))
                .filter(|(_, v)| !(self.hide_missing && v.starts_with("--")))
                .collect();
            if items.is_empty() {
                continue;
            }
            ui.strong(*title);
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for (k, v) in items {
                    tile(ui, k, v);
                }
            });
            ui.add_space(10.0);
        }
    }

    fn maxmin_ui(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        header(ui, "Max / Min", &mut refresh);
        if refresh {
            self.send(Cmd::MaxMin);
        }
        ui.checkbox(&mut self.metric, "Metric units");
        ui.add_space(6.0);
        let u = self.units();
        let f = |v: &Option<MmValue>| v.as_ref().map(|v| view::fmt_mm(v, &u)).unwrap_or_default();
        egui::Grid::new("maxmin").num_columns(3).striped(true).spacing([24.0, 6.0]).show(ui, |ui| {
            ui.strong("");
            ui.strong("Max");
            ui.strong("Min");
            ui.end_row();
            for it in &self.maxmin {
                ui.label(it.name);
                ui.label(f(&it.max));
                ui.label(f(&it.min));
                ui.end_row();
            }
        });
        ui.add_space(12.0);
        if ui.button("Clear max/min memory…").clicked() {
            self.ask("Clear max/min", "Erase all recorded maximum and minimum values on the logger?", "Clear", Cmd::ClearMaxMin);
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        header(ui, "Settings", &mut refresh);
        if refresh {
            self.send(Cmd::Load(Section::Settings));
        }
        self.section_form(ui, Section::Settings, "settings");
        ui.add_space(16.0);
        ui.separator();
        ui.strong("Clock");
        ui.horizontal(|ui| {
            if ui.button("Set from this computer").on_hover_text("Sent as UTC; the logger applies its timezone").clicked() {
                self.send(Cmd::SyncTime);
            }
            if ui.button("Fetch from network").clicked() {
                self.send(Cmd::SyncNetTime);
            }
        });
    }

    fn calibration_ui(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        header(ui, "Calibration", &mut refresh);
        if refresh {
            self.send(Cmd::Load(Section::Calibration));
        }
        ui.weak("Offsets in °C, % and hPa; gains between 0.50 and 1.50.");
        ui.add_space(6.0);
        self.section_form(ui, Section::Calibration, "calibration");
        ui.add_space(16.0);
        ui.separator();
        ui.strong("Rain counters");
        ui.weak("Overwrite counters in mm. Leave a field empty to keep its current value.");
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for k in RAIN_KEYS {
                let v = self.rain.get_mut(k).expect("rain key");
                ui.label(pretty(k));
                ui.add(egui::TextEdit::singleline(v).desired_width(70.0));
                ui.add_space(8.0);
            }
        });
        let kv: Kv = self
            .rain
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| (k.to_string(), v.trim().to_string()))
            .collect();
        if ui.add_enabled(!kv.is_empty(), egui::Button::new("Overwrite rain counters…")).clicked() {
            let list = kv.iter().map(|(k, v)| format!("{k} = {v} mm")).collect::<Vec<_>>().join(", ");
            self.ask("Overwrite rain counters", &format!("Set {list}?"), "Overwrite", Cmd::SetRain(kv));
            self.rain.values_mut().for_each(String::clear);
        }
    }

    fn alarms_ui(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        header(ui, "Alarms", &mut refresh);
        if refresh {
            self.send(Cmd::Load(Section::Alarms));
        }
        ui.weak("Thresholds are metric. Enter none to clear a threshold; tick \"on\" to enable it.");
        ui.add_space(6.0);
        self.section_form(ui, Section::Alarms, "alarms");
    }

    fn mobile_ui(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        header(ui, "Mobile", &mut refresh);
        if refresh {
            self.send(Cmd::Load(Section::Mobile));
        }
        self.sim_banner(ui);
        self.section_form(ui, Section::Mobile, "mobile");
        ui.add_space(16.0);
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Send test SMS").clicked() {
                self.send(Cmd::TestSms);
            }
            ui.weak("Sends \"Weather Logger Test MSG\" to the saved phone numbers. Save changes first.");
        });
    }

    fn upload_ui(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        header(ui, "Weather-service upload", &mut refresh);
        if refresh {
            self.send(Cmd::Load(Section::Upload));
        }
        self.sim_banner(ui);
        ui.weak(
            "From the logger firmware: an interval of 0 turns all uploads off, and a service whose station ID or \
             password is empty is skipped. To stop one service, clear its station ID. The upload switch sits under \
             www.ecowitt.net as in the Windows app and is sent with every service. Saving writes all four at once.",
        );
        ui.add_space(4.0);
        ui.weak(
            "The custom server receives Wunderground-style requests: GET http://<host>:<port>/weatherstation/\
             updateweatherstation.<type>?ID=…&PASSWORD=…&tempf=…&dateutc=… (imperial units).",
        );
        ui.add_space(6.0);
        self.section_form(ui, Section::Upload, "upload");
    }

    fn sensors_ui(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        header(ui, "Wireless sensors", &mut refresh);
        if refresh {
            self.send(Cmd::Sensors);
        }
        if self.sensors.is_empty() {
            ui.spinner();
            return;
        }
        let mut action = None;
        egui::Grid::new("sensors").num_columns(5).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
            for h in ["Sensor", "ID", "Signal", "Battery", "Change ID"] {
                ui.strong(h);
            }
            ui.end_row();
            for r in &self.sensors {
                let name = SENSOR_SLOTS.iter().find(|(s, _)| *s == r.slot).map(|x| x.1).unwrap_or("?");
                ui.label(name);
                ui.monospace(r.id_text());
                ui.label(r.signal.to_string());
                ui.label(r.battery.to_string());
                ui.horizontal(|ui| {
                    let edit = self.sensor_edit.entry(r.slot).or_default();
                    ui.add(egui::TextEdit::singleline(edit).hint_text("hex ID").desired_width(90.0));
                    if ui.add_enabled(!edit.trim().is_empty(), egui::Button::new("Set")).clicked() {
                        action = Some((r.slot, edit.trim().to_string()));
                    }
                    if ui.button("Re-learn").on_hover_text("Forget the ID and register the next sensor heard").clicked() {
                        action = Some((r.slot, "learn".into()));
                    }
                });
                ui.end_row();
            }
        });
        if let Some((slot, id)) = action {
            self.send(Cmd::SetSensor(slot, id));
        }
    }

    fn sd_ui(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        header(ui, "SD card", &mut refresh);
        if refresh {
            self.send(Cmd::SdList);
        }
        if let Some(s) = self.sd_status {
            ui.label(format!("Card status: {}", sd_status_name(s)));
        }
        ui.add_space(6.0);
        if self.sd_files.is_empty() {
            ui.weak("No files.");
        }
        let mut delete = None;
        egui::Grid::new("sd").num_columns(3).striped(true).spacing([24.0, 4.0]).show(ui, |ui| {
            for (f, sel) in self.sd_files.iter().zip(self.sd_selected.iter_mut()) {
                ui.checkbox(sel, &f.name);
                ui.label(view::human_size(f.size));
                if ui.small_button("Delete…").clicked() {
                    delete = Some(f.name.clone());
                }
                ui.end_row();
            }
        });
        if let Some(name) = delete {
            self.ask("Delete file", &format!("Delete {name} from the logger's SD card? This can't be undone."), "Delete", Cmd::SdDelete(name));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Select all").clicked() {
                self.sd_selected.iter_mut().for_each(|s| *s = true);
            }
            if ui.button("Select none").clicked() {
                self.sd_selected.iter_mut().for_each(|s| *s = false);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Save to");
            ui.add(egui::TextEdit::singleline(&mut self.sd_dir).desired_width(320.0));
        });
        let wanted: Vec<SdFile> = self
            .sd_files
            .iter()
            .zip(&self.sd_selected)
            .filter(|(_, s)| **s)
            .map(|(f, _)| f.clone())
            .collect();
        let label = format!("Download {} file(s)", wanted.len());
        if ui.add_enabled(!wanted.is_empty() && self.pending == 0, egui::Button::new(label)).clicked() {
            self.send(Cmd::SdDownload(wanted, PathBuf::from(self.sd_dir.trim())));
        }
    }

    fn device_ui(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        header(ui, "Device", &mut refresh);
        let boot = self.in_bootloader();
        if refresh && !boot {
            self.send(Cmd::Info);
        }
        if boot {
            ui.label(
                RichText::new("The logger is in bootloader mode and won't start its application. Flash a firmware file below.")
                    .color(ui.visuals().warn_fg_color),
            );
            ui.add_space(8.0);
        }
        if let Some(s) = &self.status
            && !boot
        {
            egui::Grid::new("status").num_columns(2).striped(true).spacing([24.0, 6.0]).show(ui, |ui| {
                for (k, v) in view::status_rows(s) {
                    ui.label(k);
                    ui.label(v);
                    ui.end_row();
                }
            });
            ui.add_space(12.0);
        }

        if !boot {
            ui.separator();
            ui.strong("Restart");
            ui.horizontal(|ui| {
                if ui.button("Reboot…").clicked() {
                    self.ask("Reboot", "Reboot the logger?", "Reboot", Cmd::Reboot);
                }
                if ui.button("Factory reset…").clicked() {
                    self.ask(
                        "Factory reset",
                        "Restore the logger to factory settings? Your settings, calibration, alarms and upload accounts will be lost.",
                        "Factory reset",
                        Cmd::FactoryReset,
                    );
                }
            });
            ui.add_space(12.0);
        }

        ui.separator();
        ui.strong("Firmware");
        ui.horizontal(|ui| {
            if ui.button("Check for updates").clicked() {
                self.send(Cmd::CheckUpdates);
            }
        });
        if let Some((fw, dev)) = &self.latest {
            ui.add_space(4.0);
            let newer = dev.as_ref().filter(|d| !d.is_bootloader()).map(|d| Version::numeric(&fw.version) > Version::numeric(&d.version));
            ui.label(format!("Latest firmware: {}", fw.version));
            match newer {
                Some(true) => ui.label(RichText::new("An update is available.").color(ui.visuals().warn_fg_color)),
                Some(false) => ui.label("The logger is up to date."),
                None => ui.weak("Couldn't read the logger's version."),
            };
            if !fw.url.is_empty() {
                ui.hyperlink_to("Download the .hex file", &fw.url);
            }
            if !fw.notes.is_empty() {
                ui.weak(&fw.notes);
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("HEX file");
            ui.add(egui::TextEdit::singleline(&mut self.hex_path).hint_text("/path/to/WH6006CodeV1.x.x.hex").desired_width(360.0));
        });
        let path = PathBuf::from(self.hex_path.trim());
        if ui.add_enabled(path.is_file() && self.pending == 0, egui::Button::new("Flash firmware…")).clicked() {
            self.ask(
                "Flash firmware",
                "This flashing procedure was reverse-engineered and hasn't been tested on real hardware. \
                 Don't unplug the logger until it finishes. Keep the Windows app available as a fallback.\n\nFlash now?",
                "Flash",
                Cmd::FirmwareUpdate(path),
            );
        }
    }
}

fn is_error_line(l: &str) -> bool {
    ["ERROR", "FAIL", "Fail", "not inserted", "NOT INSERTED", "NOT READY", "is null"].iter().any(|e| l.contains(e))
}

impl App {
    /// Shown where uploads or SMS depend on the SIM.
    fn sim_banner(&self, ui: &mut egui::Ui) {
        let no_sim = self.sim_missing == Some(true) || self.live.as_ref().is_some_and(|l| !l.sim_present);
        if no_sim {
            ui.label(
                RichText::new(
                    "The logger's modem reports no SIM card, so it can't upload or send SMS. \
                     Check that the SIM is seated and has no PIN lock, then reboot the logger.",
                )
                .color(ui.visuals().warn_fg_color),
            );
            ui.add_space(6.0);
        }
    }

    fn log_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Device log");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clear").clicked() {
                    self.log.clear();
                    self.log_partial.clear();
                }
                if ui.add_enabled(!self.log.is_empty(), egui::Button::new("Save")).clicked() {
                    let name = if self.name.is_empty() { "wh6006" } else { self.name.as_str() };
                    let dir = PathBuf::from(self.sd_dir.trim());
                    let path = dir.join(format!("{name}Log.txt"));
                    let res = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, self.log.join("\n") + "\n"));
                    match res {
                        Ok(()) => self.set_message(format!("Log saved to {}", path.display()), false),
                        Err(e) => self.set_message(format!("Saving the log failed: {e}"), true),
                    }
                }
                if ui.add_enabled(!self.log.is_empty(), egui::Button::new("Copy")).clicked() {
                    ui.ctx().copy_text(self.log.join("\n"));
                }
                ui.checkbox(&mut self.log_errors_only, "Errors only");
            });
        });
        ui.weak(
            "The logger forwards its modem's AT commands and replies here, e.g. at start-up, during uploads and when \
             sending SMS. Reboot the logger from the Device tab to see the start-up sequence.",
        );
        ui.add_space(6.0);
        self.sim_banner(ui);
        if self.log.is_empty() {
            ui.weak(if self.connected { "Waiting for the logger to report something…" } else { "Not connected." });
            return;
        }
        let err = ui.visuals().error_fg_color;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
                for l in self.log.iter().filter(|l| !self.log_errors_only || is_error_line(l)) {
                    let t = RichText::new(l).monospace();
                    ui.label(if is_error_line(l) { t.color(err) } else { t });
                }
            });
        });
    }
}

fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, color);
}

fn tile(ui: &mut egui::Ui, label: &str, value: &str) {
    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(150.0);
        ui.vertical(|ui| {
            ui.weak(label);
            ui.label(RichText::new(value).size(20.0).strong());
        });
    });
}
