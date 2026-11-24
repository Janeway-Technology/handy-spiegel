// Handy-Spiegel – GTK4/libadwaita-Oberfläche für scrcpy.
// Spiegelt ein Android-Handy per USB/WLAN auf PC oder TV, auf wenig Latenz getrimmt.
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::rc::Rc;

const APP_ID: &str = "io.janeway.HandySpiegel";
const VERSION: &str = env!("CARGO_PKG_VERSION");

const CSS: &str = r#"
.device-icon { background: color-mix(in srgb, var(--accent-bg-color) 15%, transparent); color: var(--accent-color);
  border-radius: 10px; min-width: 36px; min-height: 36px; }
.device-row { min-height: 64px; }
.badge { border-radius: 99px; padding: 2px 10px; font-weight: 700; font-size: 8.5pt; }
.badge.ok { background: color-mix(in srgb, var(--success-color) 15%, transparent); color: var(--success-color); }
.badge.warn { background: color-mix(in srgb, var(--warning-color) 18%, transparent); color: var(--warning-color); }
.badge.idle { background: color-mix(in srgb, currentColor 8%, transparent); }
.badge.live { background: color-mix(in srgb, var(--error-color) 15%, transparent); color: var(--error-color); }
.step-num { background: color-mix(in srgb, currentColor 8%, transparent); border-radius: 99px; min-width: 24px; min-height: 24px; font-weight: 700; font-size: 9pt; }
.keys { padding: 10px 14px; }
.key { min-width: 44px; min-height: 44px; }
.bubble { background: color-mix(in srgb, var(--accent-bg-color) 15%, transparent); color: var(--accent-color); border-radius: 14px; }
.qr-frame { background: white; border-radius: 12px; padding: 24px; }
"#;

// Bildqualität: (Name, Beschreibung, max. Auflösung, FPS, Bitrate MBit)
const PRESETS: [(&str, &str, i32, i32, i32); 3] = [
    ("Flüssig", "1280 px · geringste Verzögerung – für schwache PCs und WLAN", 1280, 60, 12),
    ("Ausgewogen", "1920 px · 60 FPS – passt fast immer", 1920, 60, 24),
    ("Scharf", "Volle Handy-Auflösung · hohe Bitrate – am besten per USB", 0, 60, 40),
];
const RES: [(i32, &str); 5] = [(0, "Original"), (1920, "1920 px"), (1600, "1600 px"), (1280, "1280 px"), (1024, "1024 px")];
const FPS: [(i32, &str); 4] = [(30, "30 FPS"), (60, "60 FPS"), (90, "90 FPS"), (120, "120 FPS")];
const BITRATE: [(i32, &str); 5] = [(8, "8 MBit/s"), (12, "12 MBit/s"), (24, "24 MBit/s"), (40, "40 MBit/s"), (60, "60 MBit/s")];
// Format: (Name, Seitenverhältnis lang/kurz; 0 = aus)
const FORMATS: [(&str, f64); 5] = [
    ("Aus", 0.0),
    ("16:9", 16.0 / 9.0),
    ("18:9", 2.0),
    ("20:9", 20.0 / 9.0),
    ("4:3", 4.0 / 3.0),
];

// ------------------------------------------------------------------ Hilfsprogramme

struct Tools {
    scrcpy: PathBuf,
    adb: PathBuf,
    server: Option<PathBuf>,
    bundled: bool,
}

/// Mitgelieferte Programme (AppImage: usr/lib/handy-spiegel) bevorzugen, sonst die des Systems.
fn find_tools() -> Tools {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("HANDY_SPIEGEL_LIBDIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(bin) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        dirs.push(bin.join("../lib/handy-spiegel"));
    }
    for d in dirs {
        if d.join("scrcpy").is_file() && d.join("adb").is_file() {
            let server = Some(d.join("scrcpy-server")).filter(|p| p.is_file());
            return Tools { scrcpy: d.join("scrcpy"), adb: d.join("adb"), server, bundled: true };
        }
    }
    Tools { scrcpy: "scrcpy".into(), adb: "adb".into(), server: None, bundled: false }
}

fn spawn(tools: &Tools, prog: &PathBuf, args: &[String], extra_env: &[(&str, &str)]) -> Result<gio::Subprocess, glib::Error> {
    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE);
    if tools.bundled {
        launcher.setenv("ADB", &tools.adb, true);
    }
    if let Some(s) = &tools.server {
        launcher.setenv("SCRCPY_SERVER_PATH", s, true);
    }
    for (k, v) in extra_env {
        launcher.setenv(k, v, true);
    }
    let mut argv: Vec<OsString> = vec![prog.clone().into()];
    argv.extend(args.iter().map(OsString::from));
    let refs: Vec<&OsStr> = argv.iter().map(|s| s.as_os_str()).collect();
    launcher.spawn(&refs)
}

async fn adb(tools: &Tools, args: &[&str]) -> String {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let Ok(p) = spawn(tools, &tools.adb, &args, &[]) else { return String::new() };
    match p.communicate_future(None).await {
        Ok((Some(out), _)) => String::from_utf8_lossy(&out).into_owned(),
        _ => String::new(),
    }
}

fn random_token(len: usize) -> String {
    const CH: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    (0..len).map(|_| CH[glib::random_int_range(0, CH.len() as i32) as usize] as char).collect()
}

// ------------------------------------------------------------------ Geräte

#[derive(Clone, PartialEq, Debug)]
struct Device {
    serial: String,
    state: String,
    model: String,
}

impl Device {
    fn wifi(&self) -> bool {
        self.serial.contains(':') || self.serial.contains("._adb-tls-connect")
    }
    fn model_name(&self) -> String {
        if self.model.is_empty() { "Android-Handy".into() } else { self.model.replace('_', " ") }
    }
    fn via(&self) -> &'static str {
        if self.wifi() { "WLAN" } else { "USB" }
    }
}

#[derive(Clone, Default, Debug)]
struct Info {
    serial: String,
    name: String,
    android: String,
    battery: Option<u32>,
    size: Option<(i32, i32)>, // natürliche Ausrichtung: (kurz, lang)
}

fn parse_devices(out: &str) -> Vec<Device> {
    out.lines()
        .filter(|l| !l.starts_with('*') && !l.starts_with("List of"))
        .filter_map(|l| {
            let mut t = l.split_whitespace();
            let serial = t.next()?.to_string();
            let state = t.next()?.to_string();
            let model = t.find_map(|x| x.strip_prefix("model:")).unwrap_or("").to_string();
            Some(Device { serial, state, model })
        })
        .collect()
}

/// `adb mdns services` → (Name, Typ, Adresse)
fn parse_mdns(out: &str) -> Vec<(String, String, String)> {
    out.lines()
        .filter_map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            (t.len() >= 3 && t[1].starts_with("_adb-tls")).then(|| (t[0].to_string(), t[1].to_string(), t[2].to_string()))
        })
        .collect()
}

/// Erreichbare IPv4 des Handys: WLAN vor Hotspot vor Rest, Mobilfunk ignorieren.
fn pick_phone_ip(out: &str) -> Option<String> {
    let mut best: Option<(u8, String)> = None;
    for l in out.lines() {
        let t: Vec<&str> = l.split_whitespace().collect();
        let Some(pos) = t.iter().position(|x| *x == "inet") else { continue };
        let (iface, Some(addr)) = (t.get(1).copied().unwrap_or(""), t.get(pos + 1)) else { continue };
        let ip = addr.split('/').next().unwrap_or("").to_string();
        let rank = if iface == "wlan0" || iface.starts_with("wlan") {
            0
        } else if iface.contains("swlan") || iface.starts_with("ap") {
            1
        } else if iface.starts_with("eth") {
            2
        } else if iface == "lo" || iface.starts_with("rmnet") || iface.starts_with("dummy") || iface.starts_with("v4-") || ip.starts_with("192.0.0.") {
            continue;
        } else {
            3
        };
        if best.as_ref().map_or(true, |(r, _)| rank < *r) {
            best = Some((rank, ip));
        }
    }
    best.map(|(_, ip)| ip)
}

// ------------------------------------------------------------------ Einstellungen

struct Settings {
    kf: glib::KeyFile,
    path: PathBuf,
}

impl Settings {
    fn load() -> Self {
        let path = glib::user_config_dir().join("handy-spiegel").join("settings.ini");
        let kf = glib::KeyFile::new();
        let _ = kf.load_from_file(&path, glib::KeyFileFlags::NONE);
        Self { kf, path }
    }
    fn int(&self, k: &str, d: i32) -> i32 {
        self.kf.integer("main", k).unwrap_or(d)
    }
    fn bool(&self, k: &str, d: bool) -> bool {
        self.kf.boolean("main", k).unwrap_or(d)
    }
    fn string(&self, k: &str) -> String {
        self.kf.string("main", k).map(|s| s.to_string()).unwrap_or_default()
    }
    fn set_int(&self, k: &str, v: i32) {
        self.kf.set_integer("main", k, v);
        self.save();
    }
    fn set_bool(&self, k: &str, v: bool) {
        self.kf.set_boolean("main", k, v);
        self.save();
    }
    fn set_string(&self, k: &str, v: &str) {
        self.kf.set_string("main", k, v);
        self.save();
    }
    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = self.kf.save_to_file(&self.path);
    }
}

// ------------------------------------------------------------------ kleine Bausteine

fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}

fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}

fn icon(name: &str, size: i32) -> gtk::Image {
    gtk::Image::builder().icon_name(name).pixel_size(size).build()
}

fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

fn combo(title: &str, items: &[&str]) -> adw::ComboRow {
    adw::ComboRow::builder().title(title).model(&gtk::StringList::new(items)).build()
}

fn switch_row(title: &str, subtitle: &str, active: bool) -> adw::SwitchRow {
    adw::SwitchRow::builder().title(title).subtitle(subtitle).active(active).build()
}

fn pill_button(icon_name: &str, text: &str, classes: &[&str]) -> gtk::Button {
    let mut c = vec!["pill"];
    c.extend_from_slice(classes);
    gtk::Button::builder().child(&adw::ButtonContent::builder().icon_name(icon_name).label(text).build()).css_classes(c).hexpand(true).build()
}

/// Gerätezeile: Symbol links, Name + Details, Status-Abzeichen rechts
fn device_row() -> (adw::ActionRow, gtk::Image, gtk::Label) {
    let row = adw::ActionRow::builder().title("Suche Handy …").build();
    row.add_css_class("device-row");
    let ico = icon("phone-symbolic", 20);
    ico.add_css_class("device-icon");
    ico.set_valign(gtk::Align::Center);
    row.add_prefix(&ico);
    let badge = gtk::Label::builder().css_classes(["badge", "idle"]).valign(gtk::Align::Center).build();
    row.add_suffix(&badge);
    (row, ico, badge)
}

fn set_badge(badge: &gtk::Label, text: &str, kind: &str) {
    for c in ["ok", "warn", "idle", "live"] {
        badge.remove_css_class(c);
    }
    badge.add_css_class(kind);
    badge.set_label(text);
}

fn format_row() -> adw::ComboRow {
    let r = combo("Schwarze Ränder entfernen", &FORMATS.map(|f| f.0));
    r.set_subtitle("Format des Spiels – 16:9 passt für die meisten");
    r
}

fn zoom_row() -> adw::SpinRow {
    let r = adw::SpinRow::with_range(100.0, 150.0, 5.0);
    r.set_title("Zoom");
    r.set_subtitle("Mittig vergrößern, in Prozent");
    r
}

fn qr_texture(text: &str) -> Option<gdk::MemoryTexture> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let n = code.width();
    let colors = code.to_colors();
    let scale = 6usize;
    let size = n * scale;
    let mut px = vec![255u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            if colors[(y / scale) * n + x / scale] == qrcode::Color::Dark {
                let i = (y * size + x) * 4;
                px[i..i + 3].copy_from_slice(&[15, 23, 42]);
            }
        }
    }
    Some(gdk::MemoryTexture::new(size as i32, size as i32, gdk::MemoryFormat::R8g8b8a8, &glib::Bytes::from_owned(px), size * 4))
}

// ------------------------------------------------------------------ Oberfläche

struct Ui {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    menu_btn: gtk::Button,
    settings_btn: gtk::Button,
    // Startseite
    dev_row: adw::ActionRow,
    dev_icon: gtk::Image,
    badge: gtk::Label,
    devices: gtk::StringList,
    device_combo: adw::ComboRow,
    help: adw::PreferencesGroup,
    help_auth: adw::PreferencesGroup,
    actions: gtk::Box,
    full_btn: gtk::Button,
    quality: adw::ToggleGroup,
    quality_group: adw::PreferencesGroup,
    format_row: adw::ComboRow,
    zoom_row: adw::SpinRow,
    sound: adw::SwitchRow,
    screen_off: adw::SwitchRow,
    gamepad: adw::SwitchRow,
    wifi_row: adw::ActionRow,
    // Live
    live_row: adw::ActionRow,
    live_badge: gtk::Label,
    live_banner: adw::Banner,
    on_top: adw::SwitchRow,
    live_mode: adw::ToggleGroup,
    live_format: adw::ComboRow,
    live_zoom: adw::SpinRow,
    // Einstellungen
    prefs: adw::PreferencesDialog,
    res_row: adw::ComboRow,
    fps_row: adw::ComboRow,
    bitrate_row: adw::ComboRow,
    monitors: gtk::StringList,
    monitor_row: adw::ComboRow,
    saved_wifi_row: adw::ActionRow,
}

struct App {
    ui: Ui,
    tools: Tools,
    settings: Settings,
    demo: Option<String>,
    devices: RefCell<Vec<Device>>,
    info: RefCell<Info>,
    info_serial: RefCell<String>,
    preferred: RefCell<String>,
    monitor_list: RefCell<Vec<gdk::Monitor>>,
    running: RefCell<Option<gio::Subprocess>>,
    running_serial: RefCell<String>,
    live_full: Cell<bool>,
    stopping: Cell<bool>,
    restart: Cell<Option<bool>>,
    polling: Cell<bool>,
    updating: Cell<bool>,
    reconnect_tick: Cell<u32>,
    crop_gen: Cell<u32>,
}

fn page_box() -> gtk::Box {
    let b = vbox(24);
    b.set_margin_top(12);
    b.set_margin_bottom(24);
    b.set_margin_start(12);
    b.set_margin_end(12);
    b
}

fn scrolled(child: &gtk::Box) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&adw::Clamp::builder().maximum_size(560).child(child).build())
        .build()
}

fn build(gapp: &adw::Application) {
    if let Some(w) = gapp.active_window() {
        w.present();
        return;
    }
    let settings = Settings::load();
    let demo = std::env::var("HANDY_SPIEGEL_DEMO").ok();
    gtk::Window::set_default_icon_name(APP_ID);

    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(d) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&d, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    // ---------------- Kopfleiste
    let header = adw::HeaderBar::new();
    let menu_btn = gtk::Button::builder()
        .child(&adw::ButtonContent::builder().icon_name("go-previous-symbolic").label("Menü").build())
        .tooltip_text("Spiegeln beenden und zurück zum Menü")
        .visible(false)
        .build();
    let settings_btn = gtk::Button::builder().icon_name("emblem-system-symbolic").tooltip_text("Einstellungen").build();
    header.pack_start(&menu_btn);
    header.pack_end(&settings_btn);

    // ---------------- Startseite
    let (dev_row, dev_icon, badge) = device_row();
    let devices = gtk::StringList::new(&[]);
    let device_combo = adw::ComboRow::builder().title("Gerät").model(&devices).visible(false).build();
    let wifi_row = adw::ActionRow::builder().title("Ohne Kabel verbinden").activatable(true).build();
    wifi_row.add_prefix(&icon("network-wireless-symbolic", 16));
    wifi_row.add_suffix(&icon("go-next-symbolic", 16));
    let g_dev = adw::PreferencesGroup::new();
    g_dev.add(&dev_row);
    g_dev.add(&device_combo);
    g_dev.add(&wifi_row);

    let help = group("Einrichtung beim ersten Mal");
    let steps = [
        ("Handy per USB-Kabel anschließen", "Das Kabel muss Daten übertragen können"),
        ("Entwickleroptionen freischalten", "Einstellungen › Telefoninfo › Softwareinformationen › 7× auf „Buildnummer“"),
        ("USB-Debugging einschalten", "Einstellungen › Entwickleroptionen › USB-Debugging"),
        ("Am Handy erlauben", "„USB-Debugging zulassen?“ – mit „Immer zulassen“ bestätigen"),
    ];
    for (i, (t, s)) in steps.iter().enumerate() {
        let row = adw::ActionRow::builder().title(*t).subtitle(*s).build();
        let n = gtk::Label::builder().label((i + 1).to_string()).css_classes(["step-num"]).valign(gtk::Align::Center).build();
        row.add_prefix(&n);
        help.add(&row);
    }

    let help_auth = group("Keine Abfrage am Handy?");
    for (i, (t, sub)) in [
        ("Handy entsperren", "Die Abfrage erscheint nur auf dem entsperrten Bildschirm"),
        ("Automatischen Blocker ausschalten", "Samsung: Einstellungen › Sicherheit und Datenschutz › Automatischer Blocker"),
        ("Autorisierungen widerrufen", "Entwickleroptionen › „USB-Debugging-Autorisierungen widerrufen“, dann USB-Debugging aus- und wieder einschalten"),
    ]
    .iter()
    .enumerate()
    {
        let row = adw::ActionRow::builder().title(*t).subtitle(*sub).build();
        row.add_prefix(&gtk::Label::builder().label((i + 1).to_string()).css_classes(["step-num"]).valign(gtk::Align::Center).build());
        help_auth.add(&row);
    }
    let retry_btn = gtk::Button::builder().label("Neu verbinden").valign(gtk::Align::Center).build();
    let retry_row = adw::ActionRow::builder().title("Abfrage erneut anstoßen").subtitle("Startet die Verbindung neu – danach aufs Handy schauen").build();
    retry_row.add_suffix(&retry_btn);
    help_auth.add(&retry_row);

    let win_btn = pill_button("focus-windows-symbolic", "Im Fenster", &["suggested-action"]);
    let full_btn = pill_button("view-fullscreen-symbolic", "Vollbild", &[]);
    let actions = gtk::Box::builder().spacing(12).homogeneous(true).build();
    actions.append(&win_btn);
    actions.append(&full_btn);

    let quality_group = group("Bildqualität");
    let quality = adw::ToggleGroup::builder().homogeneous(true).can_shrink(false).build();
    for (name, ..) in PRESETS.iter() {
        quality.add(adw::Toggle::builder().label(*name).name(*name).build());
    }
    quality_group.add(&quality);
    let g_img = group("Bildausschnitt");
    let format_r = format_row();
    let zoom_r = zoom_row();
    g_img.add(&format_r);
    g_img.add(&zoom_r);

    let g_opt = group("Beim Spielen");
    let sound = switch_row("Ton am PC", "Handy-Ton kommt aus PC bzw. Fernseher", settings.bool("sound", true));
    let screen_off = switch_row("Handy-Display ausschalten", "Spart Akku – Touch läuft weiter", settings.bool("screen_off", true));
    let gamepad = switch_row("Controller durchreichen", "Controller am PC steuern das Handy", settings.bool("gamepad", true));
    g_opt.add(&sound);
    g_opt.add(&screen_off);
    g_opt.add(&gamepad);

    let g_view = group("Anzeige");
    let monitors = gtk::StringList::new(&["Automatisch"]);
    let monitor_row = adw::ComboRow::builder().title("Vollbild auf").model(&monitors).build();
    let on_top = switch_row("Handy-Bild im Vordergrund", "Verschwindet nicht hinter anderen Fenstern, auch wenn du die Bedienung anklickst", settings.bool("on_top", true));
    g_view.add(&monitor_row);
    g_view.add(&on_top);

    let home = page_box();
    home.append(&g_dev);
    home.append(&help);
    home.append(&help_auth);
    home.append(&actions);
    home.append(&quality_group);
    home.append(&g_img);
    home.append(&g_view);
    home.append(&g_opt);

    // ---------------- Live-Seite (Steuerleiste)
    let (live_row, _, live_badge) = device_row();
    let live_banner = adw::Banner::builder().title("Vollbild liegt auf diesem Bildschirm – Alt+F wechselt ins Fenster, Alt+Tab zur Bedienung").revealed(false).visible(false).build();
    let g_live = adw::PreferencesGroup::new();
    g_live.add(&live_row);

    let g_keys = group("Handy-Tasten");
    let keys = gtk::Box::builder().spacing(8).halign(gtk::Align::Center).build();
    keys.add_css_class("card");
    keys.add_css_class("keys");
    let mut key_btns = Vec::new();
    for (ico, tip, code) in [
        ("go-previous-symbolic", "Zurück", "4"),
        ("go-home-symbolic", "Home", "3"),
        ("view-app-grid-symbolic", "Übersicht", "187"),
        ("", "", ""),
        ("audio-volume-low-symbolic", "Leiser", "25"),
        ("audio-volume-muted-symbolic", "Stumm", "164"),
        ("audio-volume-high-symbolic", "Lauter", "24"),
    ] {
        if ico.is_empty() {
            keys.append(&gtk::Separator::builder().orientation(gtk::Orientation::Vertical).margin_start(6).margin_end(6).build());
            continue;
        }
        let b = gtk::Button::builder().icon_name(ico).tooltip_text(tip).css_classes(["circular", "flat", "key"]).build();
        keys.append(&b);
        key_btns.push((b, code));
    }
    g_keys.add(&keys);

    let g_live_view = group("Anzeige");
    let live_mode = adw::ToggleGroup::builder().valign(gtk::Align::Center).homogeneous(true).build();
    live_mode.add(adw::Toggle::builder().label("Fenster").name("window").build());
    live_mode.add(adw::Toggle::builder().label("Vollbild").name("full").build());
    let mode_row = adw::ActionRow::builder().title("Modus").build();
    mode_row.add_suffix(&live_mode);
    let live_format = format_row();
    let live_zoom = zoom_row();
    g_live_view.add(&mode_row);
    g_live_view.add(&live_format);
    g_live_view.add(&live_zoom);

    let stop_btn = pill_button("media-playback-stop-symbolic", "Spiegeln beenden", &["destructive-action"]);
    stop_btn.set_halign(gtk::Align::Center);
    stop_btn.set_hexpand(false);

    let live = page_box();
    live.append(&g_live);
    live.append(&live_banner);
    live.append(&g_keys);
    live.append(&g_live_view);
    live.append(&stop_btn);

    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
    stack.add_named(&scrolled(&home), Some("home"));
    stack.add_named(&scrolled(&live), Some("live"));

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&stack));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&toasts));
    let window = adw::ApplicationWindow::builder().application(gapp).title("Handy-Spiegel").default_width(480).default_height(760).content(&toolbar).build();

    // ---------------- Einstellungen-Dialog
    let prefs = adw::PreferencesDialog::new();
    prefs.set_title("Einstellungen");
    let page = adw::PreferencesPage::new();
    let g_q = adw::PreferencesGroup::builder().title("Bildqualität im Detail").description("Ändern setzt die Qualität auf „Eigene“").build();
    let res_row = combo("Auflösung", &RES.map(|r| r.1));
    let fps_row = combo("Bildrate", &FPS.map(|r| r.1));
    let bitrate_row = combo("Bitrate", &BITRATE.map(|r| r.1));
    g_q.add(&res_row);
    g_q.add(&fps_row);
    g_q.add(&bitrate_row);
    let g_wifi = adw::PreferencesGroup::builder().title("Kabellos").build();
    let forget_btn = gtk::Button::builder().label("Vergessen").valign(gtk::Align::Center).build();
    let saved_wifi_row = adw::ActionRow::builder().title("Gespeichertes Handy").build();
    saved_wifi_row.add_suffix(&forget_btn);
    g_wifi.add(&saved_wifi_row);
    let g_about = adw::PreferencesGroup::builder().title("Über").build();
    g_about.add(
        &adw::ActionRow::builder()
            .title(format!("Handy-Spiegel {VERSION}"))
            .subtitle("Oberfläche für scrcpy · Bild per H.264, Ton unkomprimiert über USB · Rechtsklick im Handy-Fenster = Zurück, Alt+F = Vollbild")
            .build(),
    );
    for g in [&g_q, &g_wifi, &g_about] {
        page.add(g);
    }
    prefs.add(&page);

    let app = Rc::new(App {
        ui: Ui {
            window: window.clone(),
            toasts,
            stack,
            menu_btn: menu_btn.clone(),
            settings_btn: settings_btn.clone(),
            dev_row,
            dev_icon,
            badge,
            devices,
            device_combo: device_combo.clone(),
            help,
            help_auth,
            actions,
            full_btn: full_btn.clone(),
            quality: quality.clone(),
            quality_group,
            format_row: format_r.clone(),
            zoom_row: zoom_r.clone(),
            sound: sound.clone(),
            screen_off: screen_off.clone(),
            gamepad: gamepad.clone(),
            wifi_row: wifi_row.clone(),
            live_row,
            live_badge,
            live_banner,
            on_top: on_top.clone(),
            live_mode: live_mode.clone(),
            live_format: live_format.clone(),
            live_zoom: live_zoom.clone(),
            prefs: prefs.clone(),
            res_row: res_row.clone(),
            fps_row: fps_row.clone(),
            bitrate_row: bitrate_row.clone(),
            monitors,
            monitor_row: monitor_row.clone(),
            saved_wifi_row,
        },
        tools: find_tools(),
        preferred: RefCell::new(settings.string("last_serial")),
        settings,
        demo,
        devices: RefCell::new(Vec::new()),
        info: RefCell::new(Info::default()),
        info_serial: RefCell::new(String::new()),
        monitor_list: RefCell::new(Vec::new()),
        running: RefCell::new(None),
        running_serial: RefCell::new(String::new()),
        live_full: Cell::new(false),
        stopping: Cell::new(false),
        restart: Cell::new(None),
        polling: Cell::new(false),
        updating: Cell::new(false),
        reconnect_tick: Cell::new(0),
        crop_gen: Cell::new(0),
    });

    // ---------------- Werte laden
    app.updating.set(true);
    let s = &app.settings;
    let (q_res, q_fps, q_br) = app.quality();
    res_row.set_selected(RES.iter().position(|r| r.0 == q_res).unwrap_or(0) as u32);
    fps_row.set_selected(FPS.iter().position(|r| r.0 == q_fps).unwrap_or(1) as u32);
    bitrate_row.set_selected(BITRATE.iter().position(|r| r.0 == q_br).unwrap_or(2) as u32);
    match s.int("preset", 1) {
        i @ 0..=2 => quality.set_active(i as u32),
        _ => quality.set_active(gtk::INVALID_LIST_POSITION),
    }
    let fmt = (s.int("format", 0).max(0) as u32).min(FORMATS.len() as u32 - 1);
    let zm = s.int("zoom", 100) as f64;
    for r in [&format_r, &live_format] {
        r.set_selected(fmt);
    }
    for z in [&zoom_r, &live_zoom] {
        z.set_value(zm);
    }
    app.updating.set(false);
    app.update_preset_desc();
    app.update_saved_wifi();

    // ---------------- Signale
    let a = app.clone();
    quality.connect_active_notify(move |q| {
        if !a.updating.get() && q.active() < PRESETS.len() as u32 {
            a.apply_preset(q.active() as usize);
        }
    });
    for row in [&res_row, &fps_row, &bitrate_row] {
        let a = app.clone();
        row.connect_selected_notify(move |_| {
            if !a.updating.get() {
                a.custom_quality();
            }
        });
    }
    for (r, key) in [(&sound, "sound"), (&screen_off, "screen_off"), (&gamepad, "gamepad"), (&on_top, "on_top")] {
        let a = app.clone();
        r.connect_active_notify(move |r| a.settings.set_bool(key, r.is_active()));
    }
    // Format/Zoom: Start- und Live-Seite synchron halten, im Live-Betrieb neu starten
    for r in [&format_r, &live_format] {
        let a = app.clone();
        r.connect_selected_notify(move |r| {
            if a.updating.get() {
                return;
            }
            a.updating.set(true);
            a.ui.format_row.set_selected(r.selected());
            a.ui.live_format.set_selected(r.selected());
            a.updating.set(false);
            a.settings.set_int("format", r.selected() as i32);
            a.crop_changed();
        });
    }
    for z in [&zoom_r, &live_zoom] {
        let a = app.clone();
        z.connect_value_notify(move |z| {
            if a.updating.get() {
                return;
            }
            a.updating.set(true);
            a.ui.zoom_row.set_value(z.value());
            a.ui.live_zoom.set_value(z.value());
            a.updating.set(false);
            a.settings.set_int("zoom", z.value().round() as i32);
            a.crop_changed_later();
        });
    }
    let a = app.clone();
    live_mode.connect_active_notify(move |m| {
        if a.updating.get() {
            return;
        }
        a.restart_with(m.active() == 1);
    });
    let a = app.clone();
    device_combo.connect_selected_notify(move |d| {
        if a.updating.get() {
            return;
        }
        if let Some(dev) = a.devices.borrow().get(d.selected() as usize) {
            *a.preferred.borrow_mut() = dev.serial.clone();
        }
        a.update_status();
    });
    let a = app.clone();
    monitor_row.connect_selected_notify(move |r| {
        if a.updating.get() {
            return;
        }
        let name = r.selected().checked_sub(1).and_then(|i| a.monitor_list.borrow().get(i as usize).and_then(|m| m.connector())).map(|s| s.to_string()).unwrap_or_default();
        a.settings.set_string("monitor", &name);
        a.update_status();
    });
    let a = app.clone();
    forget_btn.connect_clicked(move |_| {
        a.settings.set_string("wifi_ip", "");
        a.update_saved_wifi();
    });

    let a = app.clone();
    retry_btn.connect_clicked(move |b| {
        let (a, b) = (a.clone(), b.clone());
        glib::spawn_future_local(async move {
            b.set_sensitive(false);
            let ip = a.settings.string("wifi_ip");
            if !ip.is_empty() {
                adb(&a.tools, &["disconnect", &format!("{ip}:5555")]).await;
            }
            adb(&a.tools, &["kill-server"]).await;
            glib::timeout_future_seconds(1).await;
            adb(&a.tools, &["start-server"]).await;
            a.toast("Verbindung neu gestartet – jetzt aufs entsperrte Handy schauen");
            b.set_sensitive(true);
            a.refresh();
        });
    });
    let a = app.clone();
    win_btn.connect_clicked(move |_| a.start(false));
    let a = app.clone();
    full_btn.connect_clicked(move |_| a.start(true));
    let a = app.clone();
    stop_btn.connect_clicked(move |_| a.stop());
    let a = app.clone();
    menu_btn.connect_clicked(move |_| a.stop());
    for (btn, code) in key_btns {
        let a = app.clone();
        btn.connect_clicked(move |_| a.key(code));
    }
    let a = app.clone();
    settings_btn.connect_clicked(move |_| a.ui.prefs.present(Some(&a.ui.window)));
    let a = app.clone();
    wifi_row.connect_activated(move |_| a.wifi_dialog());
    let a = app.clone();
    window.connect_close_request(move |_| {
        a.stop();
        glib::Propagation::Proceed
    });

    // Monitore
    app.update_monitors();
    if let Some(display) = gdk::Display::default() {
        let a = app.clone();
        display.monitors().connect_items_changed(move |_, _, _, _| a.update_monitors());
    }

    if app.demo.is_some() {
        app.load_demo();
    } else {
        app.update_status();
        app.refresh();
        let a = app.clone();
        glib::timeout_add_seconds_local(2, move || {
            a.refresh();
            glib::ControlFlow::Continue
        });
        let a = app.clone();
        glib::timeout_add_seconds_local(30, move || {
            a.info_serial.borrow_mut().clear(); // Akku etc. neu holen
            a.update_status();
            glib::ControlFlow::Continue
        });
    }

    window.present();
}

impl App {
    fn toast(&self, text: &str) {
        let t = adw::Toast::new(&glib::markup_escape_text(text));
        t.set_timeout(5);
        self.ui.toasts.add_toast(t);
    }

    fn selected(&self) -> Option<Device> {
        self.devices.borrow().get(self.ui.device_combo.selected() as usize).cloned()
    }

    // ---------------- Qualität
    fn quality(&self) -> (i32, i32, i32) {
        let s = &self.settings;
        match s.int("preset", 1) {
            i @ 0..=2 => {
                let p = PRESETS[i as usize];
                (p.2, p.3, p.4)
            }
            _ => (s.int("max_size", 1920), s.int("fps", 60), s.int("bitrate", 24)),
        }
    }

    fn apply_preset(&self, i: usize) {
        let p = PRESETS[i];
        self.settings.set_int("preset", i as i32);
        self.updating.set(true);
        self.ui.res_row.set_selected(RES.iter().position(|r| r.0 == p.2).unwrap_or(0) as u32);
        self.ui.fps_row.set_selected(FPS.iter().position(|r| r.0 == p.3).unwrap_or(1) as u32);
        self.ui.bitrate_row.set_selected(BITRATE.iter().position(|r| r.0 == p.4).unwrap_or(2) as u32);
        self.updating.set(false);
        self.update_preset_desc();
    }

    fn custom_quality(&self) {
        let s = &self.settings;
        s.set_int("max_size", RES[self.ui.res_row.selected() as usize].0);
        s.set_int("fps", FPS[self.ui.fps_row.selected() as usize].0);
        s.set_int("bitrate", BITRATE[self.ui.bitrate_row.selected() as usize].0);
        s.set_int("preset", 3);
        self.updating.set(true);
        self.ui.quality.set_active(gtk::INVALID_LIST_POSITION);
        self.updating.set(false);
        self.update_preset_desc();
    }

    fn update_preset_desc(&self) {
        let (r, f, b) = self.quality();
        let text = match self.settings.int("preset", 1) {
            i @ 0..=2 => PRESETS[i as usize].1.to_string(),
            _ => format!("Eigene: {} · {f} FPS · {b} MBit/s", if r == 0 { "Original".into() } else { format!("{r} px") }),
        };
        self.ui.quality_group.set_description(Some(&text));
    }

    // ---------------- Bildausschnitt
    /// scrcpy --crop in natürlicher Ausrichtung (Hochformat): breite:höhe:x:y
    fn crop_arg(&self) -> Option<String> {
        let (short, long) = self.info.borrow().size?;
        let ratio = FORMATS.get(self.settings.int("format", 0) as usize).map(|f| f.1).unwrap_or(0.0);
        let zoom = (self.settings.int("zoom", 100) as f64 / 100.0).clamp(1.0, 2.0);
        let mut h = long as f64;
        if ratio > 0.0 {
            h = h.min(short as f64 * ratio);
        }
        let w = short as f64 / zoom;
        h /= zoom;
        let even = |v: f64| ((v / 2.0).floor() * 2.0) as i32;
        let (w, h) = (even(w), even(h));
        if w >= short && h >= long {
            return None;
        }
        Some(format!("{w}:{h}:{}:{}", (short - w) / 2, (long - h) / 2))
    }

    fn crop_changed_later(self: &Rc<Self>) {
        let gen = self.crop_gen.get().wrapping_add(1);
        self.crop_gen.set(gen);
        let a = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
            if a.crop_gen.get() == gen {
                a.crop_changed();
            }
        });
    }

    fn crop_changed(self: &Rc<Self>) {
        if self.running.borrow().is_some() {
            self.restart_with(self.live_full.get());
        }
    }

    // ---------------- Geräte
    fn refresh(self: &Rc<Self>) {
        if self.polling.replace(true) {
            return;
        }
        let a = self.clone();
        glib::spawn_future_local(async move {
            let devs = parse_devices(&adb(&a.tools, &["devices", "-l"]).await);
            // gespeichertes WLAN-Handy bzw. gekoppeltes WLAN-Debugging alle ~10 s still neu verbinden
            let tick = a.reconnect_tick.get().wrapping_add(1);
            a.reconnect_tick.set(tick);
            if tick % 5 == 1 && !devs.iter().any(|d| d.state == "device" && d.wifi()) {
                let ip = a.settings.string("wifi_ip");
                if !ip.is_empty() {
                    adb(&a.tools, &["connect", &format!("{ip}:5555")]).await;
                }
                for (_, kind, addr) in parse_mdns(&adb(&a.tools, &["mdns", "services"]).await) {
                    if kind.starts_with("_adb-tls-connect") && !devs.iter().any(|d| d.serial == addr) {
                        adb(&a.tools, &["connect", &addr]).await;
                    }
                }
            }
            a.polling.set(false);
            if *a.devices.borrow() != devs {
                *a.devices.borrow_mut() = devs;
                a.update_devices();
            }
        });
    }

    fn update_devices(self: &Rc<Self>) {
        let devs = self.devices.borrow().clone();
        let labels: Vec<String> = devs.iter().map(|d| format!("{} · {}", d.model_name(), d.via())).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let pref = self.preferred.borrow().clone();
        let idx = devs
            .iter()
            .position(|d| d.serial == pref && d.state == "device")
            .or_else(|| devs.iter().position(|d| d.state == "device" && !d.wifi()))
            .or_else(|| devs.iter().position(|d| d.state == "device"))
            .unwrap_or(0);
        self.updating.set(true);
        self.ui.devices.splice(0, self.ui.devices.n_items(), &refs);
        self.ui.device_combo.set_selected(idx as u32);
        self.ui.device_combo.set_visible(devs.len() > 1);
        self.updating.set(false);
        self.update_status();
    }

    fn fetch_info(self: &Rc<Self>, dev: &Device) {
        if *self.info_serial.borrow() == dev.serial {
            return;
        }
        *self.info_serial.borrow_mut() = dev.serial.clone();
        let a = self.clone();
        let serial = dev.serial.clone();
        glib::spawn_future_local(async move {
            let out = adb(&a.tools, &["-s", &serial, "shell", "settings get global device_name; getprop ro.build.version.release; dumpsys battery | grep -m1 level; wm size"]).await;
            let lines: Vec<&str> = out.lines().map(str::trim).collect();
            let mut info = Info { serial: serial.clone(), ..Default::default() };
            if let Some(n) = lines.first().filter(|n| !n.is_empty() && **n != "null") {
                info.name = n.to_string();
            }
            info.android = lines.get(1).map(|s| s.to_string()).unwrap_or_default();
            info.battery = lines.iter().find_map(|l| l.strip_prefix("level:")).and_then(|v| v.trim().parse().ok());
            // „Override size“ ignorieren, physische Größe zählt
            info.size = lines.iter().find_map(|l| l.strip_prefix("Physical size:")).and_then(|v| {
                let (w, h) = v.trim().split_once('x')?;
                let (w, h): (i32, i32) = (w.parse().ok()?, h.parse().ok()?);
                Some((w.min(h), w.max(h)))
            });
            *a.info.borrow_mut() = info;
            a.update_status();
        });
    }

    fn update_monitors(&self) {
        let Some(display) = gdk::Display::default() else { return };
        let list: Vec<gdk::Monitor> = (0..display.monitors().n_items()).filter_map(|i| display.monitors().item(i).and_downcast::<gdk::Monitor>()).collect();
        let mut labels = vec![if list.len() > 1 { "Automatisch (anderer Bildschirm)".to_string() } else { "Automatisch".to_string() }];
        for m in &list {
            labels.push(monitor_name(m));
        }
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let want = self.settings.string("monitor");
        let idx = list.iter().position(|m| !want.is_empty() && m.connector().map(|c| c == want.as_str()).unwrap_or(false)).map(|i| i + 1).unwrap_or(0);
        self.updating.set(true);
        self.ui.monitors.splice(0, self.ui.monitors.n_items(), &refs);
        self.ui.monitor_row.set_selected(idx as u32);
        self.updating.set(false);
        *self.monitor_list.borrow_mut() = list;
    }

    /// Bildschirm, auf dem das Bedienfenster gerade liegt
    fn app_monitor(&self) -> Option<gdk::Monitor> {
        let surface = self.ui.window.surface()?;
        gdk::Display::default()?.monitor_at_surface(&surface)
    }

    /// Ziel für Vollbild: gewählter Bildschirm, sonst automatisch ein anderer als der der Bedienung
    fn fullscreen_monitor(&self) -> Option<gdk::Monitor> {
        if let Some(m) = self.chosen_monitor() {
            return Some(m);
        }
        let list = self.monitor_list.borrow();
        if list.len() < 2 {
            return None;
        }
        let mine = self.app_monitor();
        // größter der anderen Bildschirme (meist der Fernseher)
        list.iter()
            .filter(|m| Some(*m) != mine.as_ref())
            .max_by_key(|m| m.geometry().width() * m.geometry().height())
            .cloned()
    }

    fn chosen_monitor(&self) -> Option<gdk::Monitor> {
        let i = self.ui.monitor_row.selected().checked_sub(1)?;
        self.monitor_list.borrow().get(i as usize).cloned()
    }

    fn update_saved_wifi(&self) {
        let ip = self.settings.string("wifi_ip");
        self.ui.saved_wifi_row.set_subtitle(if ip.is_empty() { "Keins" } else { &ip });
    }

    fn update_status(self: &Rc<Self>) {
        let ui = &self.ui;
        let dev = self.selected();
        if let Some(d) = dev.as_ref().filter(|d| d.state == "device") {
            self.fetch_info(d);
        }
        let info = self.info.borrow().clone();
        let info_ok = dev.as_ref().map(|d| info.serial == d.serial).unwrap_or(false);
        let name = |d: &Device| if info_ok && !info.name.is_empty() { info.name.clone() } else { d.model_name() };
        let details = |d: &Device| {
            let mut parts = vec![d.via().to_string()];
            if info_ok && !info.android.is_empty() {
                parts.push(format!("Android {}", info.android));
            }
            if let Some(b) = info.battery.filter(|_| info_ok) {
                parts.push(format!("Akku {b} %"));
            }
            parts.join(" · ")
        };

        // help: 0 = keine, 1 = Ersteinrichtung, 2 = Abfrage fehlt
        let (kind, badge, ico, title, sub, ready, help) = match &dev {
            None => ("idle", "Nicht verbunden", "phone-symbolic", "Kein Handy gefunden".to_string(), "Per USB-Kabel anschließen".to_string(), false, 1),
            Some(d) if d.state == "unauthorized" => ("warn", "Erlauben", "dialog-password-symbolic", "Handy erkannt – noch nicht erlaubt".into(), format!("{} · am Handy „USB-Debugging zulassen?“ bestätigen", d.via()), false, 2),
            Some(d) if d.state != "device" => ("warn", "Keine Antwort", "dialog-warning-symbolic", d.model_name(), "Kabel neu einstecken".into(), false, 0),
            Some(d) => ("ok", "Bereit", "phone-symbolic", name(d), details(d), true, 0),
        };
        ui.dev_row.set_title(&glib::markup_escape_text(&title));
        ui.dev_row.set_subtitle(&glib::markup_escape_text(&sub));
        ui.dev_icon.set_icon_name(Some(ico));
        set_badge(&ui.badge, badge, kind);
        ui.help.set_visible(help == 1);
        ui.help_auth.set_visible(help == 2);
        ui.actions.set_sensitive(ready);
        ui.full_btn.set_tooltip_text(Some(&self.fullscreen_monitor().map(|m| format!("Vollbild auf {}", monitor_name(&m))).unwrap_or_else(|| "Vollbild auf diesem Bildschirm".into())));

        let saved = !self.settings.string("wifi_ip").is_empty();
        let wifi_dev = self.devices.borrow().iter().any(|d| d.state == "device" && d.wifi());
        ui.wifi_row.set_subtitle(if wifi_dev {
            "Verbunden – das Kabel kann ab"
        } else if saved {
            "Gekoppelt – verbindet sich automatisch"
        } else {
            "Einmal koppeln, dann kabellos spielen"
        });

        // Live-Seite
        let running = self.running.borrow().is_some();
        if running {
            let (r, f, _) = self.quality();
            ui.live_row.set_title(&glib::markup_escape_text(&dev.as_ref().map(name).unwrap_or_else(|| "Handy".into())));
            ui.live_row.set_subtitle(&format!(
                "{} · {} · {f} FPS",
                dev.as_ref().map(|d| d.via()).unwrap_or("USB"),
                if r == 0 { "Original".to_string() } else { format!("{r} px") }
            ));
            set_badge(&ui.live_badge, "Live", "live");
            self.updating.set(true);
            ui.live_mode.set_active(if self.live_full.get() { 1 } else { 0 });
            self.updating.set(false);
        }
        let was_live = ui.stack.visible_child_name().as_deref() == Some("live");
        if running != was_live && !ui.window.is_maximized() && !ui.window.is_fullscreen() {
            if running {
                ui.window.set_default_size(420, 600);
            } else {
                ui.window.set_default_size(480, 760);
            }
        }
        ui.stack.set_visible_child_name(if running { "live" } else { "home" });
        ui.menu_btn.set_visible(running);
        ui.settings_btn.set_visible(!running);
    }

    // ---------------- Spiegeln
    fn start(self: &Rc<Self>, fullscreen: bool) {
        let Some(dev) = self.selected().filter(|d| d.state == "device") else { return };
        if self.running.borrow().is_some() {
            return;
        }
        let (res, fps, bitrate) = self.quality();
        let ui = &self.ui;
        let mut args = vec![
            format!("--serial={}", dev.serial),
            format!("--window-title={}", self.ui.dev_row.title()),
            "--video-codec=h264".into(), // am schnellsten zu (de)kodieren
            format!("--video-bit-rate={bitrate}M"),
            format!("--max-fps={fps}"),
            "--video-buffer=0".into(), // kein Puffer = minimale Verzögerung
            "--stay-awake".into(),
            "--no-mouse-hover".into(),
            "--shortcut-mod=lalt,ralt".into(),
        ];
        if res > 0 {
            args.push(format!("--max-size={res}"));
        }
        if let Some(c) = self.crop_arg() {
            args.push(format!("--crop={c}"));
        }
        if ui.screen_off.is_active() {
            args.push("--turn-screen-off".into());
        }
        if ui.gamepad.is_active() {
            args.push("--gamepad=uhid".into());
        }
        if !ui.sound.is_active() {
            args.push("--no-audio".into());
        } else if dev.wifi() {
            args.extend(["--audio-codec=opus".into(), "--audio-buffer=50".into()]);
        } else {
            // USB: unkomprimiert = keine Kodier-Verzögerung
            args.extend(["--audio-codec=raw".into(), "--audio-buffer=30".into()]);
        }
        // Wayland erlaubt weder Fensterposition noch „immer im Vordergrund“ → dafür über XWayland
        let xwayland = std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_some();
        let mut use_x11 = false;
        let mut same_screen = false;
        if fullscreen {
            args.push("--fullscreen".into());
            let target = self.fullscreen_monitor();
            same_screen = match (&target, self.app_monitor()) {
                (Some(t), Some(mine)) => *t == mine,
                _ => self.monitor_list.borrow().len() < 2,
            };
            if let Some(mon) = target {
                let g = mon.geometry();
                args.push(format!("--window-x={}", g.x() + 40));
                args.push(format!("--window-y={}", g.y() + 40));
                use_x11 = true;
            }
        }
        // gleicher Bildschirm im Vollbild: nicht festnageln, sonst kommt man per Alt+Tab nicht mehr an die Bedienung
        if ui.on_top.is_active() && !(fullscreen && same_screen) {
            args.push("--always-on-top".into());
            use_x11 = true;
        }
        let mut env: Vec<(&str, &str)> = vec![
            // Vollbild nicht automatisch minimieren, wenn die Bedienung angeklickt wird
            ("SDL_VIDEO_MINIMIZE_ON_FOCUS_LOSS", "0"),
            // Handy-Fenster gehört im Dock/Panel zum Handy-Spiegel-Symbol
            ("SDL_APP_ID", APP_ID),
        ];
        if use_x11 && xwayland {
            env.push(("SDL_VIDEO_DRIVER", "x11"));
        }
        self.ui.live_banner.set_revealed(fullscreen && same_screen);
        self.ui.live_banner.set_visible(fullscreen && same_screen);

        let proc = match spawn(&self.tools, &self.tools.scrcpy, &args, &env) {
            Ok(p) => p,
            Err(e) => {
                self.toast(&format!("scrcpy konnte nicht starten: {}", e.message()));
                return;
            }
        };
        self.preferred.replace(dev.serial.clone());
        self.settings.set_string("last_serial", &dev.serial);
        self.running_serial.replace(dev.serial.clone());
        self.stopping.set(false);
        self.live_full.set(fullscreen);
        *self.running.borrow_mut() = Some(proc.clone());
        self.update_status();

        let a = self.clone();
        glib::spawn_future_local(async move {
            let out = proc.communicate_future(None).await.ok().and_then(|(o, _)| o).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            let _ = proc.wait_future().await;
            let ok = proc.is_successful();
            *a.running.borrow_mut() = None;
            if let Some(full) = a.restart.take() {
                a.start(full);
                return;
            }
            a.update_status();
            if !ok && !a.stopping.get() {
                let line = out.lines().rev().find(|l| l.starts_with("ERROR:")).or_else(|| out.lines().rev().find(|l| !l.trim().is_empty())).unwrap_or("Verbindung zum Handy beendet");
                let line: String = line.trim_start_matches("ERROR:").trim().chars().take(140).collect();
                eprintln!("{out}");
                a.toast(&line);
            }
            a.ui.window.present();
        });
    }

    fn stop(&self) {
        if let Some(p) = self.running.borrow().as_ref() {
            self.stopping.set(true);
            p.send_signal(15);
        }
    }

    fn restart_with(&self, fullscreen: bool) {
        if self.running.borrow().is_some() {
            self.restart.set(Some(fullscreen));
            self.stop();
        }
    }

    fn key(self: &Rc<Self>, code: &'static str) {
        let serial = self.running_serial.borrow().clone();
        if serial.is_empty() || self.demo.is_some() {
            return;
        }
        let a = self.clone();
        glib::spawn_future_local(async move {
            adb(&a.tools, &["-s", &serial, "shell", "input", "keyevent", code]).await;
        });
    }

    // ---------------- Kabellos
    fn wifi_dialog(self: &Rc<Self>) {
        let dialog = adw::Dialog::builder().title("Ohne Kabel verbinden").content_width(440).build();
        let vstack = adw::ViewStack::new();

        // Seite 1: über Kabel koppeln (adb tcpip) – geht mit jedem Android, auch im Hotspot
        let p1 = vbox(14);
        p1.set_margin_top(18);
        p1.set_margin_bottom(24);
        p1.set_margin_start(24);
        p1.set_margin_end(24);
        let b1 = icon("network-wired-symbolic", 30);
        b1.add_css_class("bubble");
        b1.set_size_request(64, 64);
        b1.set_halign(gtk::Align::Center);
        p1.append(&b1);
        let t1 = gtk::Label::builder().label("Einmal mit Kabel koppeln").css_classes(["title-3"]).build();
        p1.append(&t1);
        let d1 = gtk::Label::builder()
            .label("Klappt mit jedem Android – auch wenn das Handy den Hotspot macht. Danach kann das Kabel ab. Nach einem Neustart des Handys einmal wiederholen.")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label"])
            .build();
        p1.append(&d1);
        let usb_ok = self.devices.borrow().iter().any(|d| d.state == "device" && !d.wifi());
        let pair_btn = gtk::Button::builder().label(if usb_ok { "Jetzt koppeln" } else { "Erst per USB anschließen" }).css_classes(["pill", "suggested-action"]).halign(gtk::Align::Center).sensitive(usb_ok).build();
        p1.append(&pair_btn);
        let st1 = gtk::Label::builder().css_classes(["dim-label", "caption"]).wrap(true).justify(gtk::Justification::Center).build();
        p1.append(&st1);

        // Seite 2: WLAN-Debugging (Android 11+) per QR-Code oder Kopplungscode
        let p2 = vbox(14);
        p2.set_margin_top(18);
        p2.set_margin_bottom(24);
        p2.set_margin_start(24);
        p2.set_margin_end(24);
        let qr_name = format!("handy-spiegel-{}", random_token(6));
        let qr_pass = random_token(10);
        let d2 = gtk::Label::builder()
            .use_markup(true)
            .label("Handy und PC im <b>selben WLAN</b>. Am Handy: Entwickleroptionen › <b>WLAN-Debugging</b> › <b>Gerät mit QR-Code koppeln</b> und diesen Code scannen.")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        p2.append(&d2);
        if let Some(tex) = qr_texture(&format!("WIFI:T:ADB;S:{qr_name};P:{qr_pass};;")) {
            let pic = gtk::Picture::for_paintable(&tex);
            pic.set_can_shrink(false);
            let frame = gtk::Box::builder().css_classes(["qr-frame"]).halign(gtk::Align::Center).build();
            frame.append(&pic);
            p2.append(&frame);
        }
        let wait = hbox(8);
        wait.set_halign(gtk::Align::Center);
        let spinner = gtk::Spinner::builder().spinning(true).build();
        let st2 = gtk::Label::builder().label("Warte auf das Handy …").css_classes(["dim-label"]).build();
        wait.append(&spinner);
        wait.append(&st2);
        p2.append(&wait);
        let code_group = adw::PreferencesGroup::new();
        let code_row = adw::EntryRow::builder().title("Oder 6-stelliger Kopplungscode").show_apply_button(true).input_purpose(gtk::InputPurpose::Digits).build();
        code_group.add(&code_row);
        p2.append(&code_group);

        vstack.add_titled_with_icon(&p1, Some("cable"), "Über Kabel", "network-wired-symbolic");
        vstack.add_titled_with_icon(&p2, Some("qr"), "WLAN-Debugging", "network-wireless-symbolic");
        if !usb_ok || self.demo.as_deref() == Some("wifiqr") {
            vstack.set_visible_child_name("qr");
        }
        let switcher = adw::ViewSwitcher::builder().stack(&vstack).policy(adw::ViewSwitcherPolicy::Wide).build();
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&switcher));
        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&header);
        tv.set_content(Some(&vstack));
        dialog.set_child(Some(&tv));

        // Kabel-Koppeln
        let a = self.clone();
        let (pb, s1, dlg) = (pair_btn.clone(), st1.clone(), dialog.clone());
        pair_btn.connect_clicked(move |_| {
            let (a, pb, s1, dlg) = (a.clone(), pb.clone(), s1.clone(), dlg.clone());
            glib::spawn_future_local(async move {
                pb.set_sensitive(false);
                s1.set_label("Kopple …");
                match a.pair_tcpip().await {
                    Ok(msg) => {
                        a.toast(&msg);
                        dlg.close();
                    }
                    Err(e) => {
                        s1.set_label(&e);
                        pb.set_sensitive(true);
                    }
                }
            });
        });

        // QR / Code: mDNS abfragen, solange der Dialog offen ist
        let alive = Rc::new(Cell::new(true));
        let busy = Rc::new(Cell::new(false));
        let al = alive.clone();
        dialog.connect_closed(move |_| al.set(false));
        let (a, al, bz, s2, dlg, name, pass) = (self.clone(), alive.clone(), busy.clone(), st2.clone(), dialog.clone(), qr_name.clone(), qr_pass.clone());
        glib::timeout_add_local(std::time::Duration::from_millis(1500), move || {
            if !al.get() {
                return glib::ControlFlow::Break;
            }
            if !bz.get() {
                bz.set(true);
                let (a, bz, s2, dlg, name, pass) = (a.clone(), bz.clone(), s2.clone(), dlg.clone(), name.clone(), pass.clone());
                glib::spawn_future_local(async move {
                    let svcs = parse_mdns(&adb(&a.tools, &["mdns", "services"]).await);
                    if let Some((_, _, addr)) = svcs.iter().find(|(n, k, _)| *n == name && k.starts_with("_adb-tls-pairing")) {
                        s2.set_label("Kopple …");
                        a.finish_pairing(addr, &pass, &s2, &dlg).await;
                    }
                    bz.set(false);
                });
            }
            glib::ControlFlow::Continue
        });
        let (a, s2, dlg) = (self.clone(), st2.clone(), dialog.clone());
        code_row.connect_apply(move |row| {
            let code: String = row.text().chars().filter(|c| c.is_ascii_digit()).collect();
            let (a, s2, dlg) = (a.clone(), s2.clone(), dlg.clone());
            glib::spawn_future_local(async move {
                let svcs = parse_mdns(&adb(&a.tools, &["mdns", "services"]).await);
                let Some((_, _, addr)) = svcs.iter().find(|(_, k, _)| k.starts_with("_adb-tls-pairing")) else {
                    s2.set_label("Handy nicht gefunden – am Handy „Mit Kopplungscode koppeln“ offen lassen");
                    return;
                };
                a.finish_pairing(addr, &code, &s2, &dlg).await;
            });
        });

        if self.demo.as_deref().is_some_and(|d| d.starts_with("wifi")) {
            // Vorschaubild: Dialoginhalt direkt ins Fenster
            dialog.set_child(None::<&gtk::Widget>);
            spinner.set_spinning(false);
            self.ui.window.set_content(Some(&tv));
            return;
        }
        dialog.present(Some(&self.ui.window));
    }

    async fn finish_pairing(self: &Rc<Self>, addr: &str, secret: &str, status: &gtk::Label, dialog: &adw::Dialog) {
        let out = adb(&self.tools, &["pair", addr, secret]).await;
        if !out.contains("Successfully paired") {
            status.set_label("Kopplung fehlgeschlagen – nochmal versuchen");
            return;
        }
        status.set_label("Gekoppelt – verbinde …");
        let host = addr.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or_default();
        for _ in 0..10 {
            for (_, kind, a) in parse_mdns(&adb(&self.tools, &["mdns", "services"]).await) {
                if kind.starts_with("_adb-tls-connect") && a.starts_with(&host) {
                    if adb(&self.tools, &["connect", &a]).await.contains("connected") {
                        self.preferred.replace(a);
                        self.toast("Per WLAN verbunden");
                        dialog.close();
                        self.refresh();
                        return;
                    }
                }
            }
            glib::timeout_future_seconds(1).await;
        }
        status.set_label("Gekoppelt – verbindet sich gleich automatisch");
    }

    async fn pair_tcpip(self: &Rc<Self>) -> Result<String, String> {
        let usb = self.devices.borrow().iter().find(|d| d.state == "device" && !d.wifi()).cloned().ok_or("Kein Handy per USB")?;
        let out = adb(&self.tools, &["-s", &usb.serial, "shell", "ip -4 -o addr"]).await;
        let ip = pick_phone_ip(&out).ok_or("Handy ist weder im WLAN noch im Hotspot")?;
        adb(&self.tools, &["-s", &usb.serial, "tcpip", "5555"]).await;
        glib::timeout_future_seconds(2).await;
        let target = format!("{ip}:5555");
        for _ in 0..5 {
            let out = adb(&self.tools, &["connect", &target]).await;
            if out.contains("authenticate") {
                // erreichbar, Handy fragt nur noch nach Erlaubnis
                self.settings.set_string("wifi_ip", &ip);
                self.update_saved_wifi();
                self.preferred.replace(target);
                self.refresh();
                return Ok("Fast fertig – am Handy „Immer zulassen“ antippen".into());
            }
            if out.contains("connected") {
                self.settings.set_string("wifi_ip", &ip);
                self.update_saved_wifi();
                self.preferred.replace(target);
                self.refresh();
                return Ok("Per WLAN verbunden – das Kabel kann jetzt ab".into());
            }
            glib::timeout_future_seconds(1).await;
        }
        Err(format!("Keine Verbindung zu {ip} – sind PC und Handy im selben Netz?"))
    }

    // ---------------- Vorschau ohne Handy (für Screenshots)
    fn load_demo(self: &Rc<Self>) {
        let mode = self.demo.clone().unwrap_or_default();
        let dev = |state: &str| Device { serial: "DEMO".into(), state: state.into(), model: "SM_S931B".into() };
        let devs = match mode.as_str() {
            "none" => vec![],
            "auth" => vec![dev("unauthorized")],
            _ => vec![dev("device")],
        };
        *self.info_serial.borrow_mut() = "DEMO".into();
        *self.info.borrow_mut() = Info { serial: "DEMO".into(), name: "Galaxy S25".into(), android: "16".into(), battery: Some(82), size: Some((1080, 2340)) };
        *self.devices.borrow_mut() = devs.clone();
        self.updating.set(true);
        let labels: Vec<String> = devs.iter().map(|d| format!("{} · {}", d.model_name(), d.via())).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.ui.devices.splice(0, self.ui.devices.n_items(), &refs);
        self.updating.set(false);
        if mode == "live" {
            if let Ok(p) = gio::Subprocess::newv(&[OsStr::new("sleep"), OsStr::new("600")], gio::SubprocessFlags::NONE) {
                *self.running.borrow_mut() = Some(p);
                self.running_serial.replace("DEMO".into());
            }
        }
        self.update_status();
        let a = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || match mode.as_str() {
            "wifi" | "wifiqr" => a.wifi_dialog(),
            "settings" => {
                a.ui.prefs.present(Some(&a.ui.window));
            }
            _ => {}
        });
    }
}

fn monitor_name(m: &gdk::Monitor) -> String {
    let g = m.geometry();
    let name = m.description().or_else(|| m.model()).or_else(|| m.connector()).map(|s| s.to_string()).unwrap_or_else(|| "Bildschirm".into());
    format!("{name} ({}×{})", g.width(), g.height())
}

/// Fenster als PNG speichern (für Vorschau-Bilder ohne sichtbares Fenster)
fn screenshot(window: &adw::ApplicationWindow, path: &str) {
    let (w, h) = (window.width(), window.height());
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snap = gtk::Snapshot::new();
    paintable.snapshot(&snap, w as f64, h as f64);
    if let (Some(node), Some(renderer)) = (snap.to_node(), window.renderer()) {
        let tex = renderer.render_texture(&node, Some(&gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32)));
        if tex.save_to_png(path).is_err() {
            eprintln!("Screenshot fehlgeschlagen");
        }
    }
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "--check") {
        let t = find_tools();
        println!("handy-spiegel {VERSION}");
        println!("scrcpy: {} (mitgeliefert: {})", t.scrcpy.display(), t.bundled);
        println!("adb:    {}", t.adb.display());
        return glib::ExitCode::SUCCESS;
    }
    let shot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1).cloned());
    let app = adw::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_activate(move |gapp| {
        build(gapp);
        if let Some(path) = shot.clone() {
            let gapp = gapp.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(1800), move || {
                if let Some(w) = gapp.active_window().and_downcast::<adw::ApplicationWindow>() {
                    screenshot(&w, &path);
                }
                gapp.quit();
            });
        }
    });
    app.run_with_args::<&str>(&[])
}
