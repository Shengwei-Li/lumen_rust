#![cfg_attr(not(feature = "console"), windows_subsystem = "windows")]

mod audio;
mod camera;
mod clock;
mod overlay;
mod record;
mod region;
mod settings;
mod sys;
mod theme;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, Id, Pos2, Rect, Sense, Stroke, Vec2, ViewportCommand};

use record::{Recording, Source};
use settings::{Mode, Settings};
use theme::*;

const SIZE: Vec2 = Vec2::new(780.0, 648.0);

fn main() -> eframe::Result {
    // Headless check used during development: lumen --record <window title|display> <seconds> <out.mp4>
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 5 && args[1] == "--record" {
        cli_record(&args[2], args[3].parse().unwrap_or(5), &args[4]);
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lumen")
            .with_inner_size(SIZE)
            .with_resizable(false)
            .with_decorations(false)
            .with_icon(icon()),
        centered: true,
        ..Default::default()
    };
    eframe::run_native("Lumen", options, Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))))
}

pub fn log(message: &str) {
    if let Some(dir) = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Lumen-rs")) {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("lumen.log")) {
            let _ = std::io::Write::write_all(&mut f, format!("{}  {message}\n", sys_time()).as_bytes());
        }
    }
}

/// Development check: LUMEN_CAMERA=<name> adds the overlay, LUMEN_REGION=x,y,w,h crops a display,
/// LUMEN_PAUSE=a,b pauses from second a to second b.
fn cli_record(target: &str, secs: u64, out: &str) {
    let nums = |key: &str| {
        std::env::var(key).ok().map(|v| v.split(',').filter_map(|x| x.parse::<u32>().ok()).collect::<Vec<_>>()).unwrap_or_default()
    };
    let region = nums("LUMEN_REGION");
    let region = (region.len() == 4).then(|| [region[0], region[1], region[2], region[3]]);
    let source = if target == "display" {
        Source::Display(sys::monitors()[0].handle, region)
    } else {
        match sys::windows().into_iter().find(|w| w.full_title.contains(target)) {
            Some(w) => Source::Window(w.hwnd),
            None => return,
        }
    };
    let camera = std::env::var("LUMEN_CAMERA").ok().map(|device| record::CameraOptions {
        device,
        corner: record::Corner::BottomRight,
        share: 0.25,
    });
    let s = Settings::load();
    let opts = record::Options {
        source,
        path: out.into(),
        fps: 30,
        high: true,
        cursor: true,
        camera,
        audio: audio::Options { mic: s.mic.or_else(audio::default_input_name), gain: 1.0, denoise: true, system: true },
    };
    let rec = Recording::start(opts, Arc::new(AtomicU32::new(0)), record::Ended::new(|| {})).expect("start");
    let pause = nums("LUMEN_PAUSE");
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(secs) {
        let s = t0.elapsed().as_secs() as u32;
        rec.clock.set_paused(pause.len() == 2 && s >= pause[0] && s < pause[1]);
        std::thread::sleep(Duration::from_millis(20));
    }
    let frames = rec.frames.load(Ordering::Relaxed);
    let active = rec.clock.active().unwrap_or_default();
    let t = Instant::now();
    rec.finish().expect("finish");
    let _ = std::fs::write(
        format!("{out}.log"),
        format!("frames {frames}  active {:.2} s  finish {} ms\n", active.as_secs_f64(), t.elapsed().as_millis()),
    );
}

/// The app icon, drawn from the same 16x16 grid as the C# version.
fn icon() -> egui::IconData {
    let scale = 4;
    let size = 16 * scale;
    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..16 {
        for x in 0..16 {
            let corner = (x == 0 || x == 15) && (y == 0 || y == 15);
            let bracket = (y == 2 && (2..6).contains(&x))
                || (x == 2 && (2..6).contains(&y))
                || (y == 13 && (10..14).contains(&x))
                || (x == 13 && (10..14).contains(&y));
            let dot = (5..11).contains(&x) && (5..11).contains(&y) && !((x == 5 || x == 10) && (y == 5 || y == 10));
            let c: [u8; 4] = if corner {
                [0, 0, 0, 0]
            } else if dot {
                [255, 59, 0, 255]
            } else if bracket {
                [241, 241, 239, 255]
            } else {
                [17, 17, 17, 255]
            };
            for dy in 0..scale {
                for dx in 0..scale {
                    let i = ((y * scale + dy) * size + x * scale + dx) * 4;
                    rgba[i..i + 4].copy_from_slice(&c);
                }
            }
        }
    }
    egui::IconData { rgba, width: size as u32, height: size as u32 }
}

fn sys_time() -> String {
    // yyyy-MM-dd HH.mm from the local clock, via the Win32 API to avoid a date crate.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}.{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute)
}

enum State {
    Idle,
    Counting { done: Receiver<()>, opts: Option<record::Options>, monitor: windows::Win32::Foundation::RECT },
    Recording { rec: Recording, bar: overlay::Bar, ended: record::Ended },
    Saving { done: Receiver<(Result<PathBuf, String>, Duration)>, bar: overlay::Bar },
}

struct App {
    s: Settings,
    wins: Vec<sys::Win>,
    selected: Option<isize>,
    mics: Vec<String>,
    cameras: Vec<String>,
    level: audio::Level,
    /// What the meter shows: dB-mapped, fast attack and slow release, as in the C# MicMeter.
    shown: f32,
    meter: Option<audio::Meter>,
    meter_for: Option<String>,
    state: State,
    keys: Receiver<sys::Key>,
    key_thread: u32,
    picking: Option<Receiver<Option<[u32; 4]>>>,
    picking_monitor: usize,
    hwnd: Option<isize>,
    thumb: Option<sys::Thumb>,
    /// Display preview: texture, when it was taken, and which monitor.
    shot: Option<(egui::TextureHandle, Instant, usize)>,
    listed: Instant,
    last: Option<(PathBuf, String)>,
    notice: Option<String>,
    /// Shown once the file is saved, when the recording stopped on its own.
    after_save: Option<String>,
    styled: bool,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        theme::install(ctx);
        let (tx, rx) = channel();
        let wake = ctx.clone();
        let key_thread = sys::hotkey(tx, move || wake.request_repaint());
        let mut s = Settings::load();
        let mics = audio::input_names();
        if s.mic.as_ref().is_none_or(|m| !mics.contains(m)) {
            s.mic = audio::default_input_name().or_else(|| mics.first().cloned());
        }
        let cameras = camera::names();
        if s.camera.as_ref().is_none_or(|c| !cameras.contains(c)) {
            s.camera = cameras.first().cloned();
        }
        Self {
            s,
            wins: sys::windows(),
            selected: None,
            mics,
            cameras,
            level: Arc::new(AtomicU32::new(0)),
            shown: 0.0,
            meter: None,
            meter_for: None,
            state: State::Idle,
            keys: rx,
            key_thread,
            picking: None,
            picking_monitor: 0,
            hwnd: None,
            thumb: None,
            shot: None,
            listed: Instant::now(),
            last: None,
            notice: None,
            after_save: None,
            styled: false,
        }
    }

    fn idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }

    /// F9, the Record button and the bar's Stop all come here.
    fn toggle(&mut self, ctx: &egui::Context) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => self.begin(ctx),
            State::Recording { rec, bar, .. } => {
                bar.saving.store(true, Ordering::Relaxed);
                sys::pause_key(self.key_thread, false);
                let (tx, rx) = channel();
                let wake = ctx.clone();
                std::thread::spawn(move || {
                    let length = rec.clock.active().unwrap_or_default();
                    let _ = tx.send((rec.finish().map_err(|e| e.to_string()), length));
                    wake.request_repaint();
                });
                self.state = State::Saving { done: rx, bar };
            }
            other => self.state = other,
        }
    }

    fn begin(&mut self, ctx: &egui::Context) {
        self.notice = None;
        let mons = sys::monitors();
        let display = mons.get(self.s.display).or(mons.first());
        let (source, name, monitor) = match self.s.mode {
            Mode::Window => {
                let Some(w) = self.selected.and_then(|h| self.wins.iter().find(|w| w.hwnd == h)) else {
                    self.notice = Some("Select a window".into());
                    return;
                };
                if w.minimized {
                    self.notice = Some("The window is minimized".into());
                    return;
                }
                (Source::Window(w.hwnd), w.title.clone(), sys::monitor_of(w.hwnd))
            }
            Mode::Display => {
                let Some(m) = display else { return };
                (Source::Display(m.handle, None), format!("Display {}", self.s.display + 1), m.rect)
            }
            Mode::Region => {
                let (Some(r), Some(m)) = (self.s.region, display) else {
                    self.notice = Some("Select a region".into());
                    return;
                };
                let (mw, mh) = ((m.rect.right - m.rect.left) as u32, (m.rect.bottom - m.rect.top) as u32);
                if r[0] + r[2] > mw || r[1] + r[3] > mh {
                    self.notice = Some("Region is outside the display".into());
                    return;
                }
                (Source::Display(m.handle, Some(r)), "Region".to_string(), m.rect)
            }
        };
        if let Some(free) = sys::free_space(&self.s.out_dir)
            && free < 2 << 30
        {
            self.notice = Some("Less than 2 GB free in the output folder".into());
            return;
        }
        let _ = std::fs::create_dir_all(&self.s.out_dir);
        let clean: String = name.chars().map(|c| if "\\/:*?\"<>|".contains(c) { ' ' } else { c }).take(40).collect();
        let stamp = sys_time();
        let mut path = self.s.out_dir.join(format!("{stamp} {}.mp4", clean.trim()));
        let mut n = 2;
        while path.exists() {
            path = self.s.out_dir.join(format!("{stamp} {} ({n}).mp4", clean.trim()));
            n += 1;
        }
        // The meter's device handle is released so the recorder owns the mic.
        self.meter = None;
        self.meter_for = None;
        self.s.save();
        let opts = record::Options {
            source,
            path,
            fps: if self.s.fps60 { 60 } else { 30 },
            high: self.s.high,
            cursor: self.s.cursor,
            camera: self.s.camera_on.then(|| self.s.camera.clone()).flatten().map(|device| record::CameraOptions {
                device,
                corner: [record::Corner::TopLeft, record::Corner::TopRight, record::Corner::BottomLeft, record::Corner::BottomRight]
                    [self.s.corner],
                share: [0.18, 0.25, 0.33][self.s.camera_size],
            }),
            audio: audio::Options { mic: self.s.mic.clone(), gain: self.s.gain, denoise: self.s.denoise, system: self.s.system },
        };
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        let wake = ctx.clone();
        let done = if self.s.countdown {
            overlay::countdown(monitor, 3, move || wake.request_repaint())
        } else {
            let (tx, rx) = channel();
            let _ = tx.send(());
            rx
        };
        self.state = State::Counting { done, opts: Some(opts), monitor };
    }

    fn restore(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    /// Everything that must happen while the window is minimized: keys, the bar, start and finish.
    fn events(&mut self, ctx: &egui::Context) {
        while let Ok(key) = self.keys.try_recv() {
            match key {
                sys::Key::Record => self.toggle(ctx),
                sys::Key::Pause => {
                    if let State::Recording { rec, .. } = &self.state {
                        rec.clock.set_paused(!rec.clock.paused());
                    }
                }
            }
        }
        if let State::Recording { rec, bar, ended } = &self.state {
            let mut stop = false;
            if let Some(reason) = ended.reason() {
                self.after_save = Some(reason);
                stop = true;
            }
            while let Ok(cmd) = bar.commands.try_recv() {
                match cmd {
                    overlay::BarCommand::Pause => rec.clock.set_paused(!rec.clock.paused()),
                    overlay::BarCommand::Stop => stop = true,
                }
            }
            if stop {
                self.toggle(ctx);
            }
        }
        if let State::Counting { done, opts, monitor } = &mut self.state
            && done.try_recv().is_ok()
        {
            let (opts, monitor) = (opts.take().unwrap(), *monitor);
            let wake = ctx.clone();
            let ended = record::Ended::new(move || wake.request_repaint());
            match Recording::start(opts, self.level.clone(), ended.clone()) {
                Ok(rec) => {
                    let wake = ctx.clone();
                    let bar = overlay::Bar::show(monitor, rec.clock.clone(), self.level.clone(), move || wake.request_repaint());
                    sys::pause_key(self.key_thread, true);
                    self.state = State::Recording { rec, bar, ended };
                }
                Err(e) => {
                    self.notice = Some(format!("Could not start: {e}"));
                    self.state = State::Idle;
                    self.restore(ctx);
                }
            }
        }
        if let State::Saving { done, .. } = &self.state
            && let Ok((res, length)) = done.try_recv()
        {
            if let State::Saving { bar, .. } = std::mem::replace(&mut self.state, State::Idle) {
                bar.close();
            }
            match res {
                Ok(p) => {
                    let bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                    let size = if bytes >= 1 << 30 {
                        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
                    } else if bytes >= 10 << 20 {
                        format!("{:.0} MB", bytes as f64 / (1u64 << 20) as f64)
                    } else {
                        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
                    };
                    let t = length.as_secs();
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let text = format!("{name}  ·  {:02}:{:02}:{:02}  ·  {size}", t / 3600, t / 60 % 60, t % 60);
                    self.last = Some((p, text));
                }
                Err(e) => self.notice = Some(format!("Recording failed: {e}")),
            }
            if let Some(reason) = self.after_save.take() {
                self.notice = Some(reason);
            }
            self.restore(ctx);
        }
        if let Some(rx) = &self.picking
            && let Ok(res) = rx.try_recv()
        {
            if res.is_some() {
                self.s.region = res;
                self.s.display = self.picking_monitor;
                self.s.save();
            }
            self.picking = None;
            self.restore(ctx);
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.events(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // eframe shows the window only after the first frame, so look until it is there.
        if self.hwnd.is_none() {
            self.hwnd = sys::own_window();
        }
        if !self.styled
            && let Some(h) = self.hwnd
        {
            if std::env::var_os("LUMEN_SHOW_IN_CAPTURE").is_none() {
                sys::exclude_from_capture();
            }
            sys::frame_window(h);
            self.styled = true;
        }
        if self.idle() && self.meter_for != self.s.mic {
            self.meter = self.s.mic.as_deref().map(|m| audio::Meter::open(m, self.level.clone()));
            self.meter_for = self.s.mic.clone();
        }
        // Keep the list current without a click; rebuilt only when something actually changed.
        if self.idle() && self.s.mode == Mode::Window && self.listed.elapsed() > Duration::from_secs(2) {
            let now = sys::windows();
            if now != self.wins {
                self.wins = now;
            }
            self.listed = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_millis(50));

        let full = Rect::from_min_size(Pos2::ZERO, SIZE);
        ui.painter().rect_filled(full, 0.0, PAPER);
        let busy = !self.idle();

        // Rows: title 44, line, body, line, footer 40. Columns: 440, line, rest.
        let title = Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE.x, 44.0));
        let body = Rect::from_min_max(Pos2::new(0.0, 45.0), Pos2::new(SIZE.x, SIZE.y - 41.0));
        let footer = Rect::from_min_max(Pos2::new(0.0, SIZE.y - 40.0), Pos2::new(SIZE.x, SIZE.y));
        let p = ui.painter();
        p.rect_filled(Rect::from_min_size(Pos2::new(0.0, 44.0), Vec2::new(SIZE.x, 1.0)), 0.0, LINE);
        p.rect_filled(Rect::from_min_size(Pos2::new(0.0, body.max.y), Vec2::new(SIZE.x, 1.0)), 0.0, LINE);
        p.rect_filled(Rect::from_min_max(Pos2::new(440.0, body.min.y), Pos2::new(441.0, body.max.y)), 0.0, LINE);

        self.title_bar(ui, title);
        let left = Rect::from_min_max(Pos2::new(24.0, body.min.y + 20.0), Pos2::new(416.0, body.max.y - 20.0));
        let right = Rect::from_min_max(Pos2::new(465.0, body.min.y + 20.0), Pos2::new(SIZE.x - 24.0, body.max.y - 24.0));
        self.source(ui, left, busy);
        self.controls(ui, right, busy);
        self.footer(ui, Rect::from_min_max(footer.min + Vec2::new(24.0, 0.0), footer.max - Vec2::new(24.0, 0.0)), busy);
        // The frame DWM draws around an undecorated window is gone; a hairline keeps the edge.
        ui.painter().rect_stroke(full, 0.0, Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
    }

    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(PAPER).to_array()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.s.save();
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Recording { rec, bar, .. } => {
                let _ = rec.finish();
                bar.close();
            }
            State::Saving { done, bar } => {
                let _ = done.recv();
                bar.close();
            }
            _ => {}
        }
    }
}

// ---- layout ------------------------------------------------------------------------------------

impl App {
    fn title_bar(&mut self, ui: &mut egui::Ui, r: Rect) {
        let drag = ui.interact(Rect::from_min_max(r.min, Pos2::new(r.max.x - 92.0, r.max.y)), Id::NULL.with("drag"), Sense::click_and_drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        let p = ui.painter();
        p.text(Pos2::new(24.0, r.center().y - 1.5), Align2::LEFT_CENTER, "Lumen", semibold(14.0), INK);
        p.text(Pos2::new(r.max.x - 92.0 - 18.0, r.center().y), Align2::RIGHT_CENTER, "F9  Record    F10  Pause", mono(11.0), MUTED);
        for (i, glyph) in ["\u{E921}", "\u{E8BB}"].into_iter().enumerate() {
            let b = Rect::from_min_size(Pos2::new(r.max.x - 92.0 + 46.0 * i as f32, 0.0), Vec2::new(46.0, 44.0));
            let resp = ui.interact(b, Id::NULL.with(("chrome", i)), Sense::click());
            let close = i == 1;
            let (bg, fg) = match (resp.hovered(), close) {
                (true, true) => (Some(INK), PAPER),
                (true, false) => (Some(SURFACE), INK),
                _ => (None, INK),
            };
            let p = ui.painter();
            if let Some(bg) = bg {
                p.rect_filled(b, 0.0, bg);
            }
            p.text(b.center(), Align2::CENTER_CENTER, glyph, icons(10.0), fg);
            if resp.clicked() {
                ui.ctx().send_viewport_cmd(if close { ViewportCommand::Close } else { ViewportCommand::Minimized(true) });
            }
        }
    }

    fn source(&mut self, ui: &mut egui::Ui, area: Rect, busy: bool) {
        // Rows: header 28, 14, preview 214, 16, list.
        let head = Rect::from_min_size(area.min, Vec2::new(area.width(), 28.0));
        section(ui, head, "01", "Source");
        let seg = Rect::from_min_max(Pos2::new(head.max.x - 186.0, head.min.y), head.max);
        let mut mode = self.s.mode as usize;
        if segmented(ui, seg, "mode", &["Window", "Display", "Region"], &mut mode, !busy) {
            self.s.mode = [Mode::Window, Mode::Display, Mode::Region][mode];
            self.wins = sys::windows();
        }
        let well = Rect::from_min_size(area.min + Vec2::new(0.0, 42.0), Vec2::new(area.width(), 214.0));
        self.preview(ui, well);

        let list = Rect::from_min_max(Pos2::new(area.min.x, well.max.y + 16.0), area.max);
        if self.s.mode == Mode::Region {
            ui.painter().rect_filled(Rect::from_min_size(list.min + Vec2::new(0.0, 24.0), Vec2::new(list.width(), 1.0)), 0.0, LINE);
            let (clicked, b) = outline(ui, list.min + Vec2::new(0.0, 45.0), "Select Region", !busy && self.picking.is_none());
            if clicked {
                // Like the C# version: the window steps aside and the region is drawn on the display under the cursor.
                let mons = sys::monitors();
                let i = sys::monitor_at_cursor(&mons);
                if let Some(m) = mons.get(i) {
                    self.picking_monitor = i;
                    ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
                    let wake = ui.ctx().clone();
                    self.picking = Some(region::select(m.rect, move || wake.request_repaint()));
                }
            }
            if let Some([x, y, w, h]) = self.s.region {
                ui.painter().text(Pos2::new(b.max.x + 16.0, b.center().y), Align2::LEFT_CENTER, format!("{w} × {h}  at {x}, {y}"), mono(11.0), MUTED);
            }
            return;
        }

        let windows = self.s.mode == Mode::Window;
        let mons = monitors();
        let count = if windows { self.wins.len() } else { mons.len() };
        let label = format!("{}  {count}", if windows { "Windows" } else { "Displays" });
        ui.painter().text(list.min + Vec2::new(0.0, 7.0), Align2::LEFT_CENTER, label, mono(11.0), MUTED);
        if windows {
            let w = text_width(ui, "Refresh", &sans(12.0));
            if ghost(ui, Pos2::new(list.max.x - w, list.min.y + 8.0), "Refresh", !busy).0 {
                self.wins = sys::windows();
            }
        }
        let rows = Rect::from_min_max(list.min + Vec2::new(0.0, 24.0), list.max);
        ui.painter().rect_filled(Rect::from_min_size(rows.min, Vec2::new(rows.width(), 1.0)), 0.0, LINE);
        let inner = Rect::from_min_max(rows.min + Vec2::new(0.0, 1.0), rows.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if windows {
                    let mut pick = None;
                    for w in &self.wins {
                        let meta = if w.minimized { format!("{} · minimized", w.app) } else { w.app.clone() };
                        if row(ui, &w.title, &meta, &w.full_title, self.selected == Some(w.hwnd), !busy) {
                            pick = Some(w.hwnd);
                        }
                    }
                    if pick.is_some() {
                        self.selected = pick;
                    }
                } else {
                    for (i, (name, w, h)) in mons.iter().enumerate() {
                        if row(ui, name, &format!("{w} × {h}"), name, self.s.display == i, !busy) && self.s.display != i {
                            self.s.display = i;
                            self.s.region = None;
                        }
                    }
                }
            });
        });
    }

    /// Live picture of the source: a DWM thumbnail for a window, a twice-a-second grab for a display.
    fn preview(&mut self, ui: &mut egui::Ui, well: Rect) {
        ui.painter().rect_filled(well, 0.0, SURFACE);
        let inset = well.shrink(10.0);
        let ppp = ui.ctx().pixels_per_point();
        let note = |ui: &egui::Ui, main: &str, sub: &str| {
            let p = ui.painter();
            if main.is_empty() {
                p.text(well.center(), Align2::CENTER_CENTER, sub, sans(12.0), MUTED);
            } else if sub.is_empty() {
                p.text(well.center(), Align2::CENTER_CENTER, main, mono(22.0), INK);
            } else {
                p.text(well.center() - Vec2::new(0.0, 10.0), Align2::CENTER_CENTER, main, mono(22.0), INK);
                p.text(well.center() + Vec2::new(0.0, 16.0), Align2::CENTER_CENTER, sub, sans(12.0), MUTED);
            }
        };
        let mut thumb_shown = false;

        match self.s.mode {
            Mode::Window => match self.selected.and_then(|h| self.wins.iter().find(|w| w.hwnd == h)) {
                None => note(ui, "", "No window selected"),
                Some(w) if w.minimized => note(ui, "Minimized", &w.app),
                Some(w) => {
                    if self.thumb.as_ref().is_none_or(|t| t.source != w.hwnd) {
                        self.thumb = self.hwnd.and_then(|me| sys::Thumb::new(me, w.hwnd));
                    }
                    if let Some(t) = &self.thumb {
                        let (sw, sh) = t.source_size();
                        if sw > 0 && sh > 0 {
                            let r = fit(inset, sw as f32, sh as f32);
                            t.show(Some(windows::Win32::Foundation::RECT {
                                left: (r.min.x * ppp).round() as i32,
                                top: (r.min.y * ppp).round() as i32,
                                right: (r.max.x * ppp).round() as i32,
                                bottom: (r.max.y * ppp).round() as i32,
                            }));
                            thumb_shown = true;
                        }
                    }
                }
            },
            Mode::Display | Mode::Region => {
                let mons = sys::monitors();
                if let Some(m) = mons.get(self.s.display).or(mons.first()) {
                    let stale = self.shot.as_ref().is_none_or(|(_, at, d)| *d != self.s.display || at.elapsed() > Duration::from_millis(500));
                    if stale && let Some((w, h, rgba)) = sys::grab(m.rect, (inset.width() * ppp) as u32) {
                        let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                        match &mut self.shot {
                            Some((tex, at, d)) => {
                                tex.set(image, egui::TextureOptions::LINEAR);
                                *at = Instant::now();
                                *d = self.s.display;
                            }
                            None => {
                                let tex = ui.ctx().load_texture("preview", image, egui::TextureOptions::LINEAR);
                                self.shot = Some((tex, Instant::now(), self.s.display));
                            }
                        }
                    }
                    if let Some((tex, ..)) = &self.shot {
                        let (mw, mh) = ((m.rect.right - m.rect.left) as f32, (m.rect.bottom - m.rect.top) as f32);
                        let r = fit(inset, mw, mh);
                        let p = ui.painter();
                        p.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                        if self.s.mode == Mode::Region {
                            match self.s.region {
                                Some([x, y, w, h]) => {
                                    let k = r.width() / mw;
                                    let sel = Rect::from_min_size(r.min + Vec2::new(x as f32, y as f32) * k, Vec2::new(w as f32, h as f32) * k);
                                    let dim = Color32::from_rgba_unmultiplied(17, 17, 17, 150);
                                    for band in [
                                        Rect::from_min_max(r.min, Pos2::new(r.max.x, sel.min.y)),
                                        Rect::from_min_max(Pos2::new(r.min.x, sel.max.y), r.max),
                                        Rect::from_min_max(Pos2::new(r.min.x, sel.min.y), Pos2::new(sel.min.x, sel.max.y)),
                                        Rect::from_min_max(Pos2::new(sel.max.x, sel.min.y), Pos2::new(r.max.x, sel.max.y)),
                                    ] {
                                        p.rect_filled(band, 0.0, dim);
                                    }
                                    p.rect_stroke(sel, 0.0, Stroke::new(1.0, PAPER), egui::StrokeKind::Outside);
                                }
                                None => {
                                    p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(228, 228, 225, 200));
                                    note(ui, "", "No region selected");
                                }
                            }
                        }
                    }
                }
            }
        }
        if !thumb_shown && let Some(t) = &self.thumb {
            t.show(None);
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui, area: Rect, busy: bool) {
        let x0 = area.min.x;
        let w = area.width();
        let at = |y: f32, h: f32| Rect::from_min_size(Pos2::new(x0, y), Vec2::new(w, h));
        let mut y = area.min.y;

        // 02 Camera: header 28; picker 30 after 10; position and size row 32 after 12.
        let head = at(y, 28.0);
        section(ui, head, "02", "Camera");
        switch(ui, head.right_center(), "camera", &mut self.s.camera_on, !busy && !self.cameras.is_empty());
        let cam = self.s.camera_on && !busy;
        y += 38.0;
        picker(ui, at(y, 30.0), "camera", &self.cameras, &mut self.s.camera, "No camera", cam);
        y += 42.0;
        let r = at(y, 32.0);
        let label = ui.painter().text(r.left_center(), Align2::LEFT_CENTER, "Position", sans(12.0), fade(MUTED, cam, 0.4));
        corners(ui, Rect::from_min_size(Pos2::new(label.max.x + 10.0, r.min.y), Vec2::new(48.0, 32.0)), &mut self.s.corner, cam);
        let size = Rect::from_min_size(Pos2::new(r.max.x - 102.0, r.center().y - 14.0), Vec2::new(102.0, 28.0));
        ui.painter().text(Pos2::new(size.min.x - 10.0, r.center().y), Align2::RIGHT_CENTER, "Size", sans(12.0), fade(MUTED, cam, 0.4));
        segmented(ui, size, "camsize", &["S", "M", "L"], &mut self.s.camera_size, cam);
        y += 32.0 + 26.0;

        // 03 Audio: header 28; picker 30 after 10; meter 6 after 10; gain, noise, system after 12 each.
        section(ui, at(y, 28.0), "03", "Audio");
        y += 38.0;
        picker(ui, at(y, 30.0), "mic", &self.mics, &mut self.s.mic, "No microphone", !busy);
        y += 40.0;
        let peak = f32::from_bits(self.level.load(Ordering::Relaxed)) * self.s.gain;
        let db = if peak <= 1e-5 { -60.0 } else { 20.0 * peak.log10() };
        let target = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        self.shown = if target > self.shown { target } else { self.shown * 0.88 + target * 0.12 };
        meter(ui, at(y, 6.0), self.shown, 36);
        y += 18.0;
        let gain = at(y, 18.0);
        let label = ui.painter().text(gain.left_center(), Align2::LEFT_CENTER, "Gain", sans(12.0), MUTED);
        let track = Rect::from_min_max(Pos2::new(label.max.x + 14.0, gain.min.y), Pos2::new(gain.max.x - 44.0, gain.max.y));
        fader(ui, track, "gain", &mut self.s.gain, (0.5, 2.0), 0.05, !busy);
        ui.painter().text(gain.right_center(), Align2::RIGHT_CENTER, format!("{:.0}%", self.s.gain * 100.0), mono(11.0), INK);
        y += 30.0;
        switch_row(ui, at(y, 16.0), "Noise Suppression", &mut self.s.denoise, !busy);
        y += 28.0;
        switch_row(ui, at(y, 16.0), "System Audio", &mut self.s.system, !busy);
        y += 16.0 + 26.0;

        // 04 Output: header 28; two segmented halves 28 after 10; Cursor | Countdown after 14.
        section(ui, at(y, 28.0), "04", "Output");
        y += 38.0;
        let half = (w - 12.0) / 2.0;
        let mut fps = self.s.fps60 as usize;
        if segmented(ui, Rect::from_min_size(Pos2::new(x0, y), Vec2::new(half, 28.0)), "fps", &["30 fps", "60 fps"], &mut fps, !busy) {
            self.s.fps60 = fps == 1;
        }
        let mut q = self.s.high as usize;
        if segmented(ui, Rect::from_min_size(Pos2::new(x0 + half + 12.0, y), Vec2::new(half, 28.0)), "quality", &["Standard", "High"], &mut q, !busy) {
            self.s.high = q == 1;
        }
        y += 42.0;
        switch_row(ui, Rect::from_min_size(Pos2::new(x0, y), Vec2::new(half, 16.0)), "Cursor", &mut self.s.cursor, !busy);
        switch_row(ui, Rect::from_min_size(Pos2::new(x0 + half + 12.0, y), Vec2::new(half, 16.0)), "Countdown", &mut self.s.countdown, !busy);

        // Record: 52 px at the bottom of the column.
        let btn = Rect::from_min_max(Pos2::new(x0, area.max.y - 52.0), area.max);
        let resp = ui.interact(btn, Id::NULL.with("record"), Sense::click());
        let pressed = resp.is_pointer_button_down_on();
        let p = ui.painter();
        p.rect_filled(btn, 0.0, if pressed { Color32::from_rgb(0x3A, 0x3A, 0x3A) } else if resp.hovered() { INK_HOVER } else { INK });
        let label = match &self.state {
            State::Idle => "Record",
            State::Counting { .. } => "Starting",
            State::Recording { .. } => "Stop",
            State::Saving { .. } => "Saving",
        };
        let dot = Pos2::new(btn.min.x + 25.0, btn.center().y);
        p.circle_filled(dot, 5.0, SIGNAL);
        p.text(Pos2::new(dot.x + 17.0, btn.center().y - 0.5), Align2::LEFT_CENTER, label, semibold(14.0), PAPER);
        p.text(Pos2::new(btn.max.x - 20.0, btn.center().y), Align2::RIGHT_CENTER, "F9", mono(11.0), MUTED_ON_INK);
        if resp.clicked() {
            let ctx = ui.ctx().clone();
            self.toggle(&ctx);
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui, r: Rect, busy: bool) {
        let y = r.center().y;
        let label = ui.painter().text(Pos2::new(r.min.x, y), Align2::LEFT_CENTER, "Save to", sans(12.0), MUTED);
        let path = self.s.out_dir.display().to_string();
        let (shown, cut) = trim(ui, &path, &mono(11.0), 300.0);
        let dir = ui.painter().text(Pos2::new(label.max.x + 10.0, y), Align2::LEFT_CENTER, shown, mono(11.0), MUTED);
        if cut {
            tip(ui.interact(dir, Id::NULL.with("dir"), Sense::hover()), &path);
        }
        if ghost(ui, Pos2::new(dir.max.x + 12.0, y), "Change", !busy).0
            && let Some(d) = sys::pick_folder(&self.s.out_dir)
        {
            self.s.out_dir = d;
            self.s.save();
        }

        if let Some(n) = &self.notice {
            let (t, _) = trim(ui, n, &sans(12.0), 380.0);
            ui.painter().text(Pos2::new(r.max.x, y), Align2::RIGHT_CENTER, t, sans(12.0), SIGNAL);
        } else if let Some((file, text)) = self.last.clone() {
            let reveal_w = text_width(ui, "Show in Folder", &sans(12.0));
            let play_w = text_width(ui, "Play", &sans(12.0));
            let reveal_x = r.max.x - reveal_w;
            let play_x = reveal_x - 14.0 - play_w;
            if ghost(ui, Pos2::new(reveal_x, y), "Show in Folder", true).0 {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", file.display())).spawn();
            }
            if ghost(ui, Pos2::new(play_x, y), "Play", true).0 {
                let _ = std::process::Command::new("explorer").arg(&file).spawn();
            }
            let (t, cut) = trim(ui, &text, &mono(11.0), 250.0);
            let shown = ui.painter().text(Pos2::new(play_x - 16.0, y), Align2::RIGHT_CENTER, t, mono(11.0), MUTED);
            if cut {
                tip(ui.interact(shown, Id::NULL.with("last"), Sense::hover()), &text);
            }
        }
    }
}

fn monitors() -> Vec<(String, u32, u32)> {
    sys::monitors()
        .iter()
        .enumerate()
        .map(|(i, m)| (format!("Display {}", i + 1), (m.rect.right - m.rect.left) as u32, (m.rect.bottom - m.rect.top) as u32))
        .collect()
}
