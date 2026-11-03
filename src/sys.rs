//! Small Win32 helpers: window list and size, previews, capture exclusion, F9 hotkey, folder picker.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    DwmExtendFrameIntoClientArea, DwmGetWindowAttribute, DwmQueryThumbnailSourceSize, DwmSetWindowAttribute,
    DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    EnumDisplayMonitors, GetDC, GetMonitorInfoW, HALFTONE, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, HDC, HGDIOBJ, HMONITOR, ReleaseDC, SRCCOPY, SelectObject, SetStretchBltMode,
    StretchBlt,
};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_F9, VK_F10};
use windows::Win32::UI::Shell::{FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, EnumWindows, GetCursorPos, GWL_EXSTYLE, GetMessageW, GetWindowLongPtrW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, MSG, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_APP, WM_HOTKEY, WS_EX_TOOLWINDOW,
};
use windows::core::BOOL;

#[derive(Clone, PartialEq)]
pub struct Win {
    pub hwnd: isize,
    /// The title as shown: without a trailing " - App Name" that only repeats `app`.
    pub title: String,
    pub full_title: String,
    /// Executable name without extension, e.g. "chrome".
    pub app: String,
    pub minimized: bool,
}

fn app_name(pid: u32) -> String {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// "Page - Google Chrome" -> "Page" when the tail names the app the row already shows.
fn short_title(title: &str, app: &str) -> String {
    let squash = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let app = squash(app);
    for sep in [" - ", " \u{2014} ", " \u{2013} "] {
        if let Some(i) = title.rfind(sep) {
            let tail = squash(&title[i + sep.len()..]);
            if !app.is_empty() && i > 0 && (tail.contains(&app) || app.contains(&tail) && !tail.is_empty()) {
                return title[..i].trim_end().to_string();
            }
        }
    }
    title.to_string()
}

/// Top-level windows a person would pick: visible, titled, not tool windows, not cloaked, not ours.
pub fn windows() -> Vec<Win> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
        let out = unsafe { &mut *(l.0 as *mut Vec<Win>) };
        unsafe {
            if !IsWindowVisible(h).as_bool() {
                return true.into();
            }
            if GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                return true.into();
            }
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as _, 4);
            if cloaked != 0 {
                return true.into();
            }
            let mut pid = 0;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pid == std::process::id() {
                return true.into();
            }
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(h, &mut buf);
            if n == 0 {
                return true.into();
            }
            let full_title = String::from_utf16_lossy(&buf[..n as usize]);
            let app = app_name(pid);
            out.push(Win {
                hwnd: h.0 as isize,
                title: short_title(&full_title, &app),
                full_title,
                app,
                minimized: IsIconic(h).as_bool(),
            });
        }
        true.into()
    }
    let mut out = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// The visible frame of a window (without the drop shadow), in physical pixels.
pub fn window_size(hwnd: isize) -> (u32, u32) {
    let mut r = RECT::default();
    unsafe {
        let _ = DwmGetWindowAttribute(
            HWND(hwnd as _),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as _,
            size_of::<RECT>() as u32,
        );
    }
    ((r.right - r.left).max(2) as u32, (r.bottom - r.top).max(2) as u32)
}

/// A live DWM thumbnail of another window, drawn by the compositor straight onto ours.
pub struct Thumb {
    id: isize,
    pub source: isize,
}

impl Thumb {
    pub fn new(dest: isize, source: isize) -> Option<Self> {
        unsafe { DwmRegisterThumbnail(HWND(dest as _), HWND(source as _)).ok().map(|id| Self { id, source }) }
    }

    pub fn source_size(&self) -> (i32, i32) {
        unsafe { DwmQueryThumbnailSourceSize(self.id).map(|s: SIZE| (s.cx, s.cy)).unwrap_or((0, 0)) }
    }

    /// `rect` is in physical pixels of the destination's client area; `None` hides it.
    pub fn show(&self, rect: Option<RECT>) {
        let props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: rect.unwrap_or_default(),
            fVisible: rect.is_some().into(),
            opacity: 255,
            fSourceClientAreaOnly: false.into(),
            ..Default::default()
        };
        unsafe {
            let _ = DwmUpdateThumbnailProperties(self.id, &props);
        }
    }
}

impl Drop for Thumb {
    fn drop(&mut self) {
        unsafe {
            let _ = DwmUnregisterThumbnail(self.id);
        }
    }
}

/// A small RGBA picture of part of the screen. Lumen's own window is excluded from it.
pub fn grab(src: RECT, width: u32) -> Option<(u32, u32, Vec<u8>)> {
    let (sw, sh) = (src.right - src.left, src.bottom - src.top);
    if sw <= 0 || sh <= 0 {
        return None;
    }
    let w = width.max(16) as i32;
    let h = ((w as i64 * sh as i64) / sw as i64).max(1) as i32;
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let result = CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok().map(|bmp| {
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            SetStretchBltMode(mem, HALFTONE);
            let _ = StretchBlt(mem, 0, 0, w, h, Some(screen), src.left, src.top, sw, sh, SRCCOPY);
            let bgra = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            let mut rgba = Vec::with_capacity(bgra.len());
            for px in bgra.as_chunks::<4>().0 {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            (w as u32, h as u32, rgba)
        });
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    }
}

/// The UI's own top-level window.
pub fn own_window() -> Option<isize> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
        unsafe {
            // The thread also owns helper windows (IME and the like); the app window is titled "Lumen".
            let mut buf = [0u16; 16];
            let n = GetWindowTextW(h, &mut buf) as usize;
            if IsWindowVisible(h).as_bool() && String::from_utf16_lossy(&buf[..n]) == "Lumen" {
                *(l.0 as *mut isize) = h.0 as isize;
                return false.into();
            }
        }
        true.into()
    }
    let mut found = 0isize;
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(cb), LPARAM(&mut found as *mut _ as isize));
    }
    (found != 0).then_some(found)
}

/// Keeps every window of the UI thread out of screen captures.
pub fn exclude_from_capture() {
    unsafe extern "system" fn cb(h: HWND, _: LPARAM) -> BOOL {
        unsafe {
            let _ = SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE);
        }
        true.into()
    }
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(cb), LPARAM(0));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Record,
    Pause,
}

const WM_PAUSE_KEY: u32 = WM_APP + 1;

/// Global hotkeys on their own thread: F9 always, F10 only while recording (see `pause_key`).
/// Returns the thread id that `pause_key` talks to.
pub fn hotkey(tx: Sender<Key>, wake: impl Fn() + Send + 'static) -> u32 {
    let (id_tx, id_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || unsafe {
        let mut msg = MSG::default();
        // Creates the message queue before the id is handed out.
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        let _ = id_tx.send(GetCurrentThreadId());
        let _ = RegisterHotKey(None, 1, MOD_NOREPEAT, VK_F9.0 as u32);
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            match msg.message {
                WM_HOTKEY => {
                    let key = if msg.wParam.0 == 2 { Key::Pause } else { Key::Record };
                    if tx.send(key).is_ok() {
                        wake();
                    }
                }
                WM_PAUSE_KEY if msg.wParam.0 == 1 => {
                    let _ = RegisterHotKey(None, 2, MOD_NOREPEAT, VK_F10.0 as u32);
                }
                WM_PAUSE_KEY => {
                    let _ = UnregisterHotKey(None, 2);
                }
                _ => {}
            }
        }
    });
    id_rx.recv().unwrap_or(0)
}

/// F10 is claimed only while recording, so other apps keep it the rest of the time.
pub fn pause_key(thread: u32, on: bool) {
    unsafe {
        let _ = PostThreadMessageW(thread, WM_PAUSE_KEY, WPARAM(on as usize), LPARAM(0));
    }
}

pub struct Mon {
    pub handle: isize,
    pub rect: RECT,
}

/// Monitors in EnumDisplayMonitors order, the same order windows-capture uses.
pub fn monitors() -> Vec<Mon> {
    unsafe extern "system" fn cb(m: HMONITOR, _: HDC, r: *mut RECT, l: LPARAM) -> BOOL {
        let out = unsafe { &mut *(l.0 as *mut Vec<Mon>) };
        out.push(Mon { handle: m.0 as isize, rect: unsafe { *r } });
        true.into()
    }
    let mut out = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// The monitor a window is on, as a rect in physical pixels.
pub fn monitor_of(hwnd: isize) -> RECT {
    unsafe {
        let m = MonitorFromWindow(HWND(hwnd as _), MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(m, &mut info);
        info.rcMonitor
    }
}

/// Index into `mons` of the monitor under the mouse.
pub fn monitor_at_cursor(mons: &[Mon]) -> usize {
    let mut p = windows::Win32::Foundation::POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    mons.iter()
        .position(|m| p.x >= m.rect.left && p.x < m.rect.right && p.y >= m.rect.top && p.y < m.rect.bottom)
        .unwrap_or(0)
}

/// Free bytes on the drive holding `dir` (or its nearest existing parent).
pub fn free_space(dir: &std::path::Path) -> Option<u64> {
    let mut d = dir;
    while !d.exists() {
        d = d.parent()?;
    }
    let mut free = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(&windows::core::HSTRING::from(d.as_os_str()), Some(&mut free), None, None).ok()?;
    }
    Some(free)
}

/// Square corners and the DWM shadow for the undecorated main window, as in the C# version.
pub fn frame_window(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as _);
        let pref = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as _, size_of_val(&pref) as u32);
        let margins = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 0, cyBottomHeight: 1 };
        let _ = DwmExtendFrameIntoClientArea(h, &margins);
    }
}

pub fn pick_folder(start: &std::path::Path) -> Option<PathBuf> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dlg.SetOptions(dlg.GetOptions().ok()? | FOS_PICKFOLDERS).ok()?;
        if let Ok(item) = windows::Win32::UI::Shell::SHCreateItemFromParsingName::<_, _, windows::Win32::UI::Shell::IShellItem>(
            &windows::core::HSTRING::from(start.as_os_str()),
            None,
        ) {
            let _ = dlg.SetFolder(&item);
        }
        dlg.Show(None).ok()?;
        let path = dlg.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = path.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(path.0 as _));
        s.map(PathBuf::from)
    }
}
