//! Region selection: a dimmed overlay over one monitor; drag to select, Shift for 16:9, Esc to cancel.
//! The selected area is painted in the colour key, so it shows through undimmed.

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush, DeleteDC,
    DeleteObject, DrawTextW, DT_CENTER, DT_SINGLELINE, DT_VCENTER, EndPaint, FillRect, FrameRect, HGDIOBJ,
    InvalidateRect, PAINTSTRUCT, SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, VK_ESCAPE, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, IDC_CROSS, LWA_ALPHA, LWA_COLORKEY,
    LoadCursorW, MSG, PostQuitMessage, RegisterClassW, SW_SHOW, SetForegroundWindow, SetLayeredWindowAttributes,
    ShowWindow, TranslateMessage, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const KEY: COLORREF = COLORREF(0x00FF00FF); // magenta, never drawn otherwise
const INK: COLORREF = COLORREF(0x00111111);
const PAPER: COLORREF = COLORREF(0x00EFF1F1);
const MIN: i32 = 64;

struct State {
    from: Option<POINT>,
    to: POINT,
    result: Option<[u32; 4]>,
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State { from: None, to: POINT { x: 0, y: 0 }, result: None }) };
}

fn point(l: LPARAM) -> POINT {
    POINT { x: (l.0 & 0xFFFF) as i16 as i32, y: ((l.0 >> 16) & 0xFFFF) as i16 as i32 }
}

/// Normalized selection; with Shift the height follows the width at 16:9. Even sizes for H.264.
fn selection(s: &State) -> Option<RECT> {
    let a = s.from?;
    let mut b = s.to;
    if unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0 {
        let w = (b.x - a.x).abs();
        let h = w * 9 / 16;
        b.y = if b.y >= a.y { a.y + h } else { a.y - h };
    }
    let (l, r) = (a.x.min(b.x), a.x.max(b.x));
    let (t, bt) = (a.y.min(b.y), a.y.max(b.y));
    Some(RECT { left: l, top: t, right: l + ((r - l) & !1), bottom: t + ((bt - t) & !1) })
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_LBUTTONDOWN => {
                STATE.with_borrow_mut(|s| {
                    s.from = Some(point(lp));
                    s.to = point(lp);
                });
                SetCapture(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                STATE.with_borrow_mut(|s| s.to = point(lp));
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                STATE.with_borrow_mut(|s| {
                    s.to = point(lp);
                    if let Some(r) = selection(s)
                        && r.right - r.left >= MIN
                        && r.bottom - r.top >= MIN
                    {
                        s.result = Some([r.left as u32, r.top as u32, (r.right - r.left) as u32, (r.bottom - r.top) as u32]);
                    }
                    s.from = None;
                });
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            // Esc or a right click cancels (the overlay may not get keyboard focus).
            WM_RBUTTONUP => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_KEYDOWN if wp.0 == VK_SHIFT.0 as usize => {
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_PAINT => {
                paint(hwnd);
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

unsafe fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client);
        let (w, h) = (client.right, client.bottom);
        // Draw off-screen, then blit once: no flicker while dragging.
        let mem = CreateCompatibleDC(Some(dc));
        let bmp = CreateCompatibleBitmap(dc, w, h);
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        let ink = CreateSolidBrush(INK);
        let key = CreateSolidBrush(KEY);
        let paper = CreateSolidBrush(PAPER);
        FillRect(mem, &client, ink);

        let font = CreateFontW(-(h / 60).max(14), 0, 0, 0, 400, 0, 0, 0, Default::default(), Default::default(), Default::default(), Default::default(), 0, w!("Segoe UI"));
        let old_font = SelectObject(mem, HGDIOBJ(font.0));
        SetBkMode(mem, TRANSPARENT);
        SetTextColor(mem, PAPER);

        let sel = STATE.with_borrow(selection);
        if let Some(r) = sel {
            FillRect(mem, &r, key);
            let frame = RECT { left: r.left - 2, top: r.top - 2, right: r.right + 2, bottom: r.bottom + 2 };
            FrameRect(mem, &frame, paper);
            let mut label: Vec<u16> = format!("{} × {}", r.right - r.left, r.bottom - r.top).encode_utf16().collect();
            let line = (h / 60).max(14) + 12;
            let mut lr = RECT { left: r.left, top: r.bottom + 8, right: r.right.max(r.left + 200), bottom: r.bottom + 8 + line };
            if lr.bottom > h {
                lr = RECT { left: r.left, top: r.top - 8 - line, right: lr.right, bottom: r.top - 8 };
            }
            DrawTextW(mem, &mut label, &mut lr, DT_SINGLELINE | DT_VCENTER);
        } else {
            let mut hint: Vec<u16> = "Drag to select     Shift 16:9     Esc Cancel".encode_utf16().collect();
            let mut hr = RECT { left: 0, top: h / 2 - 30, right: w, bottom: h / 2 + 30 };
            DrawTextW(mem, &mut hint, &mut hr, DT_SINGLELINE | DT_CENTER | DT_VCENTER);
        }

        let _ = BitBlt(dc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old_font);
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteObject(HGDIOBJ(ink.0));
        let _ = DeleteObject(HGDIOBJ(key.0));
        let _ = DeleteObject(HGDIOBJ(paper.0));
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
    }
}

/// Opens the overlay on its own thread; the result (x, y, w, h relative to the monitor) arrives later.
pub fn select(monitor: RECT, wake: impl Fn() + Send + 'static) -> Receiver<Option<[u32; 4]>> {
    let (tx, rx): (Sender<_>, _) = channel();
    std::thread::spawn(move || unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = w!("LumenRegion");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        STATE.with_borrow_mut(|s| *s = State { from: None, to: POINT::default(), result: None });
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW,
            class,
            PCWSTR::null(),
            WS_POPUP,
            monitor.left,
            monitor.top,
            monitor.right - monitor.left,
            monitor.bottom - monitor.top,
            None,
            None,
            Some(instance.into()),
            None,
        ) else {
            let _ = tx.send(None);
            return;
        };
        let _ = SetLayeredWindowAttributes(hwnd, KEY, 140, LWA_ALPHA | LWA_COLORKEY);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = tx.send(STATE.with_borrow(|s| s.result));
        wake();
    });
    rx
}
