//! Webcam frames through Media Foundation. The newest frame is kept as top-down BGRA for the
//! compositor; older ones are simply overwritten.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::thread::JoinHandle;

use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFActivate, IMFAttributes, IMFMediaSource, IMFSourceReader, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_VERSION, MFCreateAttributes, MFCreateMediaType, MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources,
    MFMediaType_Video, MFSTARTUP_FULL, MFStartup, MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::core::PWSTR;

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub version: u64,
}

pub type Latest = Arc<Mutex<Option<Image>>>;

/// Media Foundation is started once per process; every caller runs on its own MTA thread.
fn startup() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
    });
}

fn devices() -> windows::core::Result<Vec<IMFActivate>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        startup();
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.unwrap();
        attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attrs, &mut list, &mut count)?;
        let mut out = Vec::new();
        for i in 0..count as usize {
            if let Some(a) = (*list.add(i)).take() {
                out.push(a);
            }
        }
        CoTaskMemFree(Some(list as _));
        Ok(out)
    }
}

fn name(a: &IMFActivate) -> Option<String> {
    unsafe {
        let mut p = PWSTR::null();
        let mut len = 0u32;
        a.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut p, &mut len).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

/// Camera names. Enumerated on a worker thread: COM is joined as MTA there, which must not
/// happen on the UI thread (the folder dialog needs it STA).
pub fn names() -> Vec<String> {
    std::thread::spawn(|| devices().map(|d| d.iter().filter_map(name).collect()).unwrap_or_default())
        .join()
        .unwrap_or_default()
}

pub struct Camera {
    pub latest: Latest,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Camera {
    pub fn open(device: &str) -> Self {
        let latest: Latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let (latest, stop, device) = (latest.clone(), stop.clone(), device.to_string());
            move || {
                if let Err(e) = run(&device, &latest, &stop) {
                    crate::log(&format!("camera {device}: {e}"));
                }
            }
        });
        Self { latest, stop, thread: Some(thread) }
    }

    pub fn close(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn reader(device: &str) -> windows::core::Result<(IMFSourceReader, IMFMediaSource)> {
    let act = devices()?
        .into_iter()
        .find(|a| name(a).as_deref() == Some(device))
        .ok_or_else(windows::core::Error::empty)?;
    unsafe {
        let source: IMFMediaSource = act.ActivateObject()?;
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.unwrap();
        // Lets the reader convert whatever the camera sends (usually MJPG or NV12) to RGB32.
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs)?;

        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

        // Prefer the camera's largest mode up to 1280 px wide; the overlay is never bigger.
        let mut best = None;
        let mut best_w = 0u32;
        for i in 0.. {
            let Ok(t) = reader.GetNativeMediaType(stream, i) else { break };
            let w = (t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0) >> 32) as u32;
            if w <= 1280 && w > best_w {
                best_w = w;
                best = Some(t);
            }
        }
        if let Some(t) = best {
            reader.SetCurrentMediaType(stream, None, &t)?;
        }
        let rgb = MFCreateMediaType()?;
        rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(stream, None, &rgb)?;
        Ok((reader, source))
    }
}

fn run(device: &str, latest: &Latest, stop: &AtomicBool) -> windows::core::Result<()> {
    let (reader, source) = reader(device)?;
    let result = read_frames(&reader, latest, stop);
    // Releases the device now (and its light), not whenever the last reference goes.
    unsafe {
        let _ = source.Shutdown();
    }
    result
}

fn read_frames(reader: &IMFSourceReader, latest: &Latest, stop: &AtomicBool) -> windows::core::Result<()> {
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let size = unsafe { reader.GetCurrentMediaType(stream)?.GetUINT64(&MF_MT_FRAME_SIZE)? };
    let (w, h) = ((size >> 32) as u32, size as u32);
    if w == 0 || h == 0 {
        return Err(windows::core::Error::empty());
    }
    // Row order when the buffer has no 2D interface: MF's default for RGB32 is top-down unless
    // the type says otherwise with a negative stride.
    let stride = unsafe { reader.GetCurrentMediaType(stream)?.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
        .map(|v| v as i32)
        .unwrap_or((w * 4) as i32);
    let row = (w * 4) as usize;
    let mut version = 0;
    while !stop.load(Ordering::Relaxed) {
        let mut flags = 0u32;
        let mut sample = None;
        unsafe { reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))? };
        let Some(sample) = sample else { continue };
        let buffer = unsafe { sample.ConvertToContiguousBuffer()? };
        let mut data = vec![0u8; row * h as usize];
        unsafe {
            if let Ok(b2d) = windows::core::Interface::cast::<IMF2DBuffer>(&buffer) {
                let mut ptr = std::ptr::null_mut();
                let mut pitch = 0i32;
                b2d.Lock2D(&mut ptr, &mut pitch)?;
                // Lock2D points at the top row; a negative pitch walks a bottom-up image upwards.
                for y in 0..h as usize {
                    let src = std::slice::from_raw_parts(ptr.offset(y as isize * pitch as isize), row);
                    data[y * row..][..row].copy_from_slice(src);
                }
                b2d.Unlock2D()?;
            } else {
                let mut ptr = std::ptr::null_mut();
                let mut len = 0u32;
                buffer.Lock(&mut ptr, None, Some(&mut len))?;
                let pitch = stride.unsigned_abs() as usize;
                if len as usize >= pitch * h as usize {
                    for y in 0..h as usize {
                        let src_row = if stride < 0 { h as usize - 1 - y } else { y };
                        let src = std::slice::from_raw_parts(ptr.add(src_row * pitch), row);
                        data[y * row..][..row].copy_from_slice(src);
                    }
                }
                buffer.Unlock()?;
            }
        }
        version += 1;
        *latest.lock().unwrap() = Some(Image { width: w, height: h, data, version });
    }
    Ok(())
}

/// Bilinear scale of a BGRA image, alpha forced opaque.
pub fn scale(src: &Image, w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (w * h * 4) as usize];
    let (sw, sh) = (src.width as usize, src.height as usize);
    let fx = (sw as f32 - 1.0) / (w.max(2) as f32 - 1.0);
    let fy = (sh as f32 - 1.0) / (h.max(2) as f32 - 1.0);
    for y in 0..h as usize {
        let v = y as f32 * fy;
        let (y0, ty) = (v as usize, v.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..w as usize {
            let u = x as f32 * fx;
            let (x0, tx) = (u as usize, u.fract());
            let x1 = (x0 + 1).min(sw - 1);
            let o = (y * w as usize + x) * 4;
            for c in 0..3 {
                let p = |xx: usize, yy: usize| src.data[(yy * sw + xx) * 4 + c] as f32;
                let top = p(x0, y0) + (p(x1, y0) - p(x0, y0)) * tx;
                let bot = p(x0, y1) + (p(x1, y1) - p(x0, y1)) * tx;
                out[o + c] = (top + (bot - top) * ty) as u8;
            }
            out[o + 3] = 255;
        }
    }
    out
}
