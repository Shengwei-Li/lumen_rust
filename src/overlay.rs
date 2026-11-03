//! The two small native windows shown while the main window is minimized: the countdown and the
//! recording bar. Both are ink squares, never take focus, and stay out of the recording.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen, CreateSolidBrush, DT_CENTER,
    DT_LEFT, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, HDC, HFONT, HGDIOBJ,
    InvalidateRect, PAINTSTRUCT, PS_SOLID, SRCCOPY, ScreenToClient, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW,
    GetWindowLongPtrW, HTCAPTION, HTCLIENT, IDC_ARROW, IDC_HAND, KillTimer, LoadCursorW, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER, SetCursor, SetTimer,
    SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, WDA_EXCLUDEFROMCAPTURE,
    WM_CLOSE, WM_DESTROY, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_SETCURSOR, WM_TIMER,
    WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::audio::Level;
use crate::clock::Clock;

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}
const INK: COLORREF = rgb(0x11, 0x11, 0x11);
const INK_HOVER: COLORREF = rgb(0x2A, 0x2A, 0x2A);
const INK_LINE: COLORREF = rgb(0x33, 0x33, 0x33);
const PAPER: COLORREF = rgb(0xF1, 0xF1, 0xEF);
const SIGNAL: COLORREF = rgb(0xFF, 0x3B, 0x00);

fn keep_out_of_capture(hwnd: HWND) {
    if std::env::var_os("LUMEN_SHOW_IN_CAPTURE").is_none() {
        unsafe {
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        }
    }
}

/// Grayscale antialiasing: ClearType fringes the glyphs with colour on the ink background.
fn font(px: i32, face: PCWSTR, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(-px, 0, 0, 0, weight, 0, 0, 0, Default::default(), Default::default(), Default::default(), ANTIALIASED_QUALITY, 0, face)
    }
}

unsafe fn text(dc: HDC, s: &str, mut r: RECT, flags: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT) {
    let mut w: Vec<u16> = s.encode_utf16().collect();
    unsafe { DrawTextW(dc, &mut w, &mut r, flags | DT_SINGLELINE | DT_VCENTER) };
}

fn fill(dc: HDC, r: RECT, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        FillRect(dc, &r, b);
        let _ = DeleteObject(HGDIOBJ(b.0));
    }
}

/// Places a window centred on the monitor, `top` px from its top edge (or centred when `None`).
/// Sizes are in 96-dpi units; placing twice picks up the target monitor's DPI after the first move.
fn place(hwnd: HWND, monitor: RECT, w: f32, h: f32, top: Option<f32>) {
    for _ in 0..2 {
        let k = scale(hwnd);
        place_px(hwnd, monitor, (w * k).round() as i32, (h * k).round() as i32, top.map(|t| (t * k) as i32));
    }
}

fn place_px(hwnd: HWND, monitor: RECT, w: i32, h: i32, top: Option<i32>) {
    let x = monitor.left + (monitor.right - monitor.left - w) / 2;
    let y = match top {
        Some(t) => monitor.top + t,
        None => monitor.top + (monitor.bottom - monitor.top - h) / 2,
    };
    unsafe {
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 }
}

fn register(class: PCWSTR, proc: windows::Win32::UI::WindowsAndMessaging::WNDPROC) -> windows::Win32::Foundation::HINSTANCE {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default().into();
        let wc = WNDCLASSW {
            lpfnWndProc: proc,
            hInstance: instance,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        instance
    }
}

fn create(class: PCWSTR, instance: windows::Win32::Foundation::HINSTANCE) -> Option<HWND> {
    unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Lumen"),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(instance),
            None,
        )
        .ok()
    }
}

fn pump() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ---- countdown ---------------------------------------------------------------------------------

struct Count {
    left: i32,
}

unsafe extern "system" fn count_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let ctx = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Count;
        match msg {
            WM_TIMER if !ctx.is_null() => {
                (*ctx).left -= 1;
                if (*ctx).left <= 0 {
                    let _ = KillTimer(Some(hwnd), 1);
                    let _ = DestroyWindow(hwnd);
                } else {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_PAINT if !ctx.is_null() => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut r = RECT::default();
                let _ = GetClientRect(hwnd, &mut r);
                fill(dc, r, INK);
                let k = scale(hwnd);
                let f = font((76.0 * k) as i32, w!("Consolas"), 400);
                let old = SelectObject(dc, HGDIOBJ(f.0));
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, PAPER);
                let up = RECT { top: r.top - (6.0 * k) as i32, bottom: r.bottom - (6.0 * k) as i32, ..r };
                text(dc, &(*ctx).left.to_string(), up, DT_CENTER);
                SelectObject(dc, old);
                let _ = DeleteObject(HGDIOBJ(f.0));
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Shows 3, 2, 1 centred on the monitor; the receiver fires once it has gone.
pub fn countdown(monitor: RECT, seconds: i32, wake: impl Fn() + Send + 'static) -> Receiver<()> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let class = w!("LumenCountdown");
        let instance = register(class, Some(count_proc));
        if let Some(hwnd) = create(class, instance) {
            let mut ctx = Box::new(Count { left: seconds });
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *ctx as *mut Count as isize);
                keep_out_of_capture(hwnd);
                place(hwnd, monitor, 168.0, 168.0, None);
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                SetTimer(Some(hwnd), 1, 1000, None);
            }
            pump();
            drop(ctx);
            // Let DWM remove the window before the first frame is captured.
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
        let _ = tx.send(());
        wake();
    });
    rx
}

// ---- recording bar -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarCommand {
    Pause,
    Stop,
}

struct BarCtx {
    clock: Arc<Clock>,
    level: Level,
    saving: Arc<AtomicBool>,
    hover: Option<BarCommand>,
    tx: Sender<BarCommand>,
    wake: Box<dyn Fn() + Send>,
}

/// Bar geometry in 96-dpi units (from RecordingBar.xaml).
mod geo {
    pub const H: f32 = 40.0;
    pub const DOT_X: f32 = 16.0;
    pub const DOT: f32 = 8.0;
    pub const TIME_X: f32 = 34.0;
    pub const TIME_W: f32 = 64.0;
    pub const METER_X: f32 = TIME_X + TIME_W + 14.0;
    pub const METER_W: f32 = 52.0;
    pub const METER_H: f32 = 8.0;
    pub const SEGMENTS: usize = 13;
    pub const DIVIDER_X: f32 = METER_X + METER_W + 16.0;
    pub const PAUSE_X: f32 = DIVIDER_X + 1.0;
    pub const BUTTON: f32 = 40.0;
    pub const STOP_X: f32 = PAUSE_X + BUTTON;
    pub const W: f32 = STOP_X + BUTTON + 4.0;
}

fn button_at(hwnd: HWND, x: i32) -> Option<BarCommand> {
    let k = scale(hwnd);
    let x = x as f32 / k;
    if (geo::PAUSE_X..geo::STOP_X).contains(&x) {
        Some(BarCommand::Pause)
    } else if (geo::STOP_X..geo::STOP_X + geo::BUTTON).contains(&x) {
        Some(BarCommand::Stop)
    } else {
        None
    }
}

unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let ctx = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut BarCtx;
        if ctx.is_null() {
            return match msg {
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            };
        }
        let ctx = &mut *ctx;
        match msg {
            // The bar drags from anywhere except its buttons.
            WM_NCHITTEST => {
                let mut p = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
                let _ = ScreenToClient(hwnd, &mut p);
                LRESULT(if button_at(hwnd, p.x).is_some() { HTCLIENT } else { HTCAPTION } as isize)
            }
            WM_SETCURSOR => {
                let cursor = if ctx.hover.is_some() { IDC_HAND } else { IDC_ARROW };
                SetCursor(LoadCursorW(None, cursor).ok());
                LRESULT(1)
            }
            WM_MOUSEMOVE => {
                let hover = button_at(hwnd, (lp.0 & 0xFFFF) as i16 as i32);
                if hover != ctx.hover {
                    ctx.hover = hover;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                ctx.hover = None;
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if let Some(cmd) = button_at(hwnd, (lp.0 & 0xFFFF) as i16 as i32)
                    && !ctx.saving.load(Ordering::Relaxed)
                    && ctx.tx.send(cmd).is_ok()
                {
                    (ctx.wake)();
                }
                LRESULT(0)
            }
            WM_TIMER => {
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_PAINT => {
                paint_bar(hwnd, ctx);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe fn paint_bar(hwnd: HWND, ctx: &BarCtx) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let screen = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let dc = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, client.right, client.bottom);
        let old_bmp = SelectObject(dc, HGDIOBJ(bmp.0));
        let k = scale(hwnd);
        let px = |v: f32| (v * k).round() as i32;
        let h = client.bottom;
        let mid = h / 2;
        fill(dc, client, INK);
        SetBkMode(dc, TRANSPARENT);

        let paused = ctx.clock.paused();
        let saving = ctx.saving.load(Ordering::Relaxed);

        // Dot: filled while recording, an outline while paused.
        let d = px(geo::DOT);
        let dot = RECT { left: px(geo::DOT_X), top: mid - d / 2, right: px(geo::DOT_X) + d, bottom: mid - d / 2 + d };
        if paused || saving {
            let pen = CreatePen(PS_SOLID, px(1.0).max(1), SIGNAL);
            let old_pen = SelectObject(dc, HGDIOBJ(pen.0));
            let hollow = windows::Win32::Graphics::Gdi::GetStockObject(windows::Win32::Graphics::Gdi::NULL_BRUSH);
            let old_brush = SelectObject(dc, hollow);
            let _ = windows::Win32::Graphics::Gdi::Ellipse(dc, dot.left, dot.top, dot.right, dot.bottom);
            SelectObject(dc, old_brush);
            SelectObject(dc, old_pen);
            let _ = DeleteObject(HGDIOBJ(pen.0));
        } else {
            let brush = CreateSolidBrush(SIGNAL);
            let pen = CreatePen(PS_SOLID, 1, SIGNAL);
            let ob = SelectObject(dc, HGDIOBJ(brush.0));
            let op = SelectObject(dc, HGDIOBJ(pen.0));
            let _ = windows::Win32::Graphics::Gdi::Ellipse(dc, dot.left, dot.top, dot.right, dot.bottom);
            SelectObject(dc, ob);
            SelectObject(dc, op);
            let _ = DeleteObject(HGDIOBJ(brush.0));
            let _ = DeleteObject(HGDIOBJ(pen.0));
        }

        let mono = font(px(14.0), w!("Consolas"), 400);
        let old_font = SelectObject(dc, HGDIOBJ(mono.0));
        SetTextColor(dc, PAPER);
        let t = ctx.clock.active().unwrap_or_default().as_secs();
        let label = if saving { "Saving".to_string() } else { format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60) };
        let tr = RECT { left: px(geo::TIME_X), top: 0, right: px(geo::METER_X), bottom: h };
        text(dc, &label, tr, DT_LEFT);
        SelectObject(dc, old_font);
        let _ = DeleteObject(HGDIOBJ(mono.0));

        // Meter: 13 segments, paper on, the top tenth in signal.
        let level = if paused || saving { 0.0 } else { f32::from_bits(ctx.level.load(Ordering::Relaxed)) };
        let db = 20.0 * level.max(1e-6).log10();
        let lit = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * geo::SEGMENTS as f32) as usize;
        let gap = 2.0;
        let seg_w = (geo::METER_W - gap * (geo::SEGMENTS - 1) as f32) / geo::SEGMENTS as f32;
        let mh = px(geo::METER_H);
        for i in 0..geo::SEGMENTS {
            let x = geo::METER_X + i as f32 * (seg_w + gap);
            let c = if i >= lit { INK_LINE } else if i >= geo::SEGMENTS * 9 / 10 { SIGNAL } else { PAPER };
            fill(dc, RECT { left: px(x), top: mid - mh / 2, right: px(x + seg_w), bottom: mid - mh / 2 + mh }, c);
        }

        fill(dc, RECT { left: px(geo::DIVIDER_X), top: px(10.0), right: px(geo::DIVIDER_X) + px(1.0).max(1), bottom: h - px(10.0) }, INK_LINE);

        let icons = font(px(12.0), w!("Segoe Fluent Icons"), 400);
        let old_font = SelectObject(dc, HGDIOBJ(icons.0));
        for (cmd, x, glyph) in [
            (BarCommand::Pause, geo::PAUSE_X, if paused { "\u{E768}" } else { "\u{E769}" }),
            (BarCommand::Stop, geo::STOP_X, "\u{E71A}"),
        ] {
            let r = RECT { left: px(x), top: 0, right: px(x + geo::BUTTON), bottom: h };
            if ctx.hover == Some(cmd) && !saving {
                fill(dc, r, INK_HOVER);
            }
            SetTextColor(dc, if saving { INK_LINE } else { PAPER });
            text(dc, glyph, r, DT_CENTER);
        }
        SelectObject(dc, old_font);
        let _ = DeleteObject(HGDIOBJ(icons.0));

        let _ = BitBlt(screen, 0, 0, client.right, client.bottom, Some(dc), 0, 0, SRCCOPY);
        SelectObject(dc, old_bmp);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(dc);
        let _ = EndPaint(hwnd, &ps);
    }
}

/// The recording bar. Clicks arrive on the receiver; `saving` greys it out while the file is written.
pub struct Bar {
    hwnd: Arc<AtomicIsize>,
    pub saving: Arc<AtomicBool>,
    pub commands: Receiver<BarCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Bar {
    pub fn show(monitor: RECT, clock: Arc<Clock>, level: Level, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let hwnd = Arc::new(AtomicIsize::new(0));
        let saving = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let (hwnd_out, saving) = (hwnd.clone(), saving.clone());
            move || {
                let class = w!("LumenBar");
                let instance = register(class, Some(bar_proc));
                let Some(hwnd) = create(class, instance) else { return };
                let mut ctx = Box::new(BarCtx { clock, level, saving, hover: None, tx, wake: Box::new(wake) });
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *ctx as *mut BarCtx as isize);
                    keep_out_of_capture(hwnd);
                    place(hwnd, monitor, geo::W, geo::H, Some(20.0));
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    SetTimer(Some(hwnd), 1, 100, None);
                }
                hwnd_out.store(hwnd.0 as isize, Ordering::Release);
                pump();
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
                drop(ctx);
            }
        });
        Self { hwnd, saving, commands: rx, thread: Some(thread) }
    }

    pub fn close(mut self) {
        let h = self.hwnd.load(Ordering::Acquire);
        if h != 0 {
            unsafe {
                let _ = PostMessageW(Some(HWND(h as _)), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
