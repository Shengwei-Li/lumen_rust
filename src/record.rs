//! One recording: capture, compositing and audio feeding one H.264 encoder.
//!
//! The capture thread only copies each new frame (cropped to the region, if any) into a shared
//! texture. A compositor thread, on its own D3D device, builds every output frame at a steady
//! rate: that copy plus the camera overlay. Steady input matters: Windows sends frames only when
//! the picture changes, and the transcoder stretches the timeline across long gaps.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Graphics::DirectX::Direct3D11::IDirect3DSurface;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIKeyedMutex, IDXGIResource, IDXGISurface};
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11SurfaceFromDXGISurface;
use windows::core::Interface;
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::encoder::{
    AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder, VideoSettingsSubType,
};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, MinimumUpdateIntervalSettings,
    SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

use crate::clock::Clock;
use crate::{audio, camera};

type Error = Box<dyn std::error::Error + Send + Sync>;
type Shared = Arc<Mutex<Option<VideoEncoder>>>;

/// Why a recording stopped on its own (window closed, encoder failed); the UI stops and says so.
#[derive(Clone)]
pub struct Ended {
    reason: Arc<Mutex<Option<String>>>,
    /// Set by `finish`: stopping the capture also raises its Closed event, which is not news.
    stopping: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Ended {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self { reason: Arc::new(Mutex::new(None)), stopping: Arc::new(AtomicBool::new(false)), wake: Arc::new(wake) }
    }

    fn set(&self, reason: String) {
        if self.stopping.load(Ordering::Relaxed) {
            return;
        }
        crate::log(&reason);
        self.reason.lock().unwrap().get_or_insert(reason);
        (self.wake)();
    }

    pub fn reason(&self) -> Option<String> {
        self.reason.lock().unwrap().clone()
    }
}

/// How long either side waits for the shared frame. Holders only queue one GPU copy, so a
/// timeout means a lost device; the frame is skipped rather than ending the recording.
const SYNC_MS: u32 = 1000;

/// The shared texture the capture thread fills; `version` counts copies.
#[derive(Clone, Copy)]
struct Shot {
    handle: isize,
    version: u64,
}

type Latest = Arc<Mutex<Option<Shot>>>;

pub enum Source {
    Window(isize),
    /// A monitor handle, and optionally a region inside it (x, y, w, h in physical pixels).
    Display(isize, Option<[u32; 4]>),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

pub struct CameraOptions {
    pub device: String,
    pub corner: Corner,
    /// Overlay height as a share of the video height.
    pub share: f32,
}

pub struct Options {
    pub source: Source,
    pub path: PathBuf,
    pub fps: u32,
    pub high: bool,
    pub cursor: bool,
    pub camera: Option<CameraOptions>,
    pub audio: audio::Options,
}

fn texture_desc(w: u32, h: u32, bind: u32, misc: u32) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind,
        CPUAccessFlags: 0,
        MiscFlags: misc,
    }
}

fn create(device: &ID3D11Device, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D, Error> {
    let mut t = None;
    unsafe { device.CreateTexture2D(desc, None, Some(&mut t))? };
    Ok(t.ok_or("texture")?)
}

// ---- capture thread ----------------------------------------------------------------------------

struct Flags {
    latest: Latest,
    frames: Arc<AtomicU64>,
    size: (u32, u32),
    crop: Option<[u32; 4]>,
    ended: Ended,
}

struct Handler {
    latest: Latest,
    frames: Arc<AtomicU64>,
    size: (u32, u32),
    crop: Option<[u32; 4]>,
    ended: Ended,
    shared: Option<(ID3D11Texture2D, IDXGIKeyedMutex, isize)>,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = Flags;
    type Error = Error;

    fn new(ctx: Context<Flags>) -> Result<Self, Error> {
        let f = ctx.flags;
        Ok(Self { latest: f.latest, frames: f.frames, size: f.size, crop: f.crop, ended: f.ended, shared: None })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, _: InternalCaptureControl) -> Result<(), Error> {
        if self.shared.is_none() {
            let desc = texture_desc(self.size.0, self.size.1, 0, D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32);
            let texture = create(frame.device(), &desc)?;
            let mutex: IDXGIKeyedMutex = texture.cast()?;
            let handle = unsafe { texture.cast::<IDXGIResource>()?.GetSharedHandle()? };
            self.shared = Some((texture, mutex, handle.0 as isize));
        }
        let (texture, mutex, handle) = self.shared.as_ref().unwrap();
        let [x, y, w, h] = self.crop.unwrap_or([0, 0, self.size.0, self.size.1]);
        // A window can be resized mid-recording; copy what overlaps the output size.
        let b = D3D11_BOX {
            left: x,
            top: y,
            front: 0,
            right: (x + w).min(frame.width()),
            bottom: (y + h).min(frame.height()),
            back: 1,
        };
        if b.right > b.left && b.bottom > b.top {
            unsafe {
                if mutex.AcquireSync(0, SYNC_MS).is_err() {
                    return Ok(());
                }
                frame.device_context().CopySubresourceRegion(texture, 0, 0, 0, 0, frame.as_raw_texture(), 0, Some(&b));
                let _ = mutex.ReleaseSync(0);
            }
        }
        let mut latest = self.latest.lock().unwrap();
        let version = latest.map_or(0, |s| s.version) + 1;
        *latest = Some(Shot { handle: *handle, version });
        self.frames.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Error> {
        self.ended.set("The window was closed".into());
        Ok(())
    }
}

// ---- compositor --------------------------------------------------------------------------------

struct Compositor {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct Overlay {
    texture: ID3D11Texture2D,
    size: (u32, u32),
    version: u64,
}

impl Compositor {
    /// Output frames in flight; the encoder reads them asynchronously, so they are not reused at once.
    const RING: usize = 6;

    fn start(
        encoder: Shared,
        latest: Latest,
        camera: Option<(camera::Latest, CameraOptions)>,
        size: (u32, u32),
        fps: u32,
        clock: Arc<Clock>,
        ended: Ended,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let stop = stop.clone();
            move || {
                if let Err(e) = Self::run(&encoder, &latest, camera, size, fps, &clock, &stop) {
                    ended.set(format!("Encoding stopped: {e}"));
                }
            }
        });
        Self { stop, thread: Some(thread) }
    }

    fn run(
        encoder: &Shared,
        latest: &Latest,
        camera: Option<(camera::Latest, CameraOptions)>,
        (w, h): (u32, u32),
        fps: u32,
        clock: &Clock,
        stop: &AtomicBool,
    ) -> Result<(), Error> {
        let (device, context): (ID3D11Device, ID3D11DeviceContext) = windows_capture::d3d11::create_d3d_device()?;
        let out_desc = texture_desc(w, h, (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32, 0);
        let mut ring = Vec::with_capacity(Self::RING);
        for _ in 0..Self::RING {
            let t = create(&device, &out_desc)?;
            let surface: IDirect3DSurface = unsafe { CreateDirect3D11SurfaceFromDXGISurface(&t.cast::<IDXGISurface>()?)? }.cast()?;
            ring.push((t, surface));
        }
        let mut opened: Option<(isize, ID3D11Texture2D, IDXGIKeyedMutex)> = None;
        let mut overlay: Option<Overlay> = None;
        let interval = Duration::from_secs_f64(1.0 / fps as f64);
        let mut next = Instant::now();
        let mut i = 0;

        loop {
            let stopping = stop.load(Ordering::Relaxed);
            let Some(shot) = *latest.lock().unwrap() else {
                if stopping {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(5));
                next = Instant::now();
                continue;
            };
            if opened.as_ref().is_none_or(|(h, ..)| *h != shot.handle) {
                let mut t: Option<ID3D11Texture2D> = None;
                unsafe { device.OpenSharedResource(HANDLE(shot.handle as _), &mut t)? };
                let t = t.ok_or("shared")?;
                let m: IDXGIKeyedMutex = t.cast()?;
                opened = Some((shot.handle, t, m));
            }
            // Paused: nothing reaches the encoder, and the clock does not advance.
            if clock.paused() && !stopping {
                std::thread::sleep(Duration::from_millis(10));
                next = Instant::now();
                continue;
            }
            let (_, screen, mutex) = opened.as_ref().unwrap();
            let (out, surface) = &ring[i % Self::RING];
            unsafe {
                // Without the lock the copy could tear; the previous frame in `out` is sent instead.
                if mutex.AcquireSync(0, SYNC_MS).is_ok() {
                    context.CopyResource(out, screen);
                    let _ = mutex.ReleaseSync(0);
                }
            }

            if let Some((cam, opts)) = &camera {
                Self::overlay(&device, &context, out, cam, opts, (w, h), &mut overlay)?;
            }

            clock.start();
            let ts = (clock.active().unwrap_or_default().as_nanos() / 100) as i64;
            if let Some(enc) = encoder.lock().unwrap().as_mut() {
                enc.send_surface(surface.clone(), ts)?;
            }
            i += 1;
            if stopping {
                return Ok(());
            }

            next += interval;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else if now - next > interval * 3 {
                next = now; // fell behind (suspend, heavy load); do not burst to catch up
            }
        }
    }

    fn overlay(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        out: &ID3D11Texture2D,
        cam: &camera::Latest,
        opts: &CameraOptions,
        (w, h): (u32, u32),
        overlay: &mut Option<Overlay>,
    ) -> Result<(), Error> {
        {
            let image = cam.lock().unwrap();
            if let Some(img) = image.as_ref()
                && overlay.as_ref().is_none_or(|o| o.version != img.version)
            {
                let oh = ((h as f32 * opts.share) as u32).max(32) & !1;
                let ow = ((oh as f32 * img.width as f32 / img.height.max(1) as f32) as u32).min(w) & !1;
                if overlay.as_ref().is_none_or(|o| o.size != (ow, oh)) {
                    *overlay = Some(Overlay { texture: create(device, &texture_desc(ow, oh, 0, 0))?, size: (ow, oh), version: 0 });
                }
                let o = overlay.as_mut().unwrap();
                let pixels = camera::scale(img, ow, oh);
                unsafe { context.UpdateSubresource(&o.texture, 0, None, pixels.as_ptr() as _, ow * 4, 0) };
                o.version = img.version;
            }
        }
        if let Some(o) = overlay.as_ref() {
            let margin = (h as f32 * 0.03) as u32;
            let (ow, oh) = o.size;
            let x = match opts.corner {
                Corner::TopLeft | Corner::BottomLeft => margin,
                _ => w.saturating_sub(ow + margin),
            };
            let y = match opts.corner {
                Corner::TopLeft | Corner::TopRight => margin,
                _ => h.saturating_sub(oh + margin),
            };
            unsafe { context.CopySubresourceRegion(out, 0, x, y, 0, &o.texture, 0, None) };
        }
        Ok(())
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ---- recording ---------------------------------------------------------------------------------

pub struct Recording {
    control: Option<CaptureControl<Handler, Error>>,
    audio: Option<audio::Pipe>,
    compositor: Option<Compositor>,
    camera: Option<camera::Camera>,
    encoder: Shared,
    ended: Ended,
    pub clock: Arc<Clock>,
    pub frames: Arc<AtomicU64>,
    pub path: PathBuf,
}

fn even(v: u32) -> u32 {
    v.max(2) & !1
}

impl Recording {
    /// Starts everything, or nothing: on failure the parts already running are stopped and the
    /// empty file is removed.
    pub fn start(opts: Options, level: audio::Level, ended: Ended) -> Result<Self, Error> {
        let (w, h, crop) = match &opts.source {
            Source::Window(hwnd) => {
                let (w, h) = crate::sys::window_size(*hwnd);
                (w, h, None)
            }
            Source::Display(_, Some([x, y, w, h])) => (*w, *h, Some([*x, *y, *w, *h])),
            Source::Display(m, None) => {
                let m = Monitor::from_raw_hmonitor(*m as _);
                (m.width()?, m.height()?, None)
            }
        };
        let (w, h) = (even(w), even(h));
        let crop = crop.map(|[x, y, ..]| [x, y, w, h]);
        let bits = (w as f64 * h as f64 * opts.fps as f64 * if opts.high { 0.08 } else { 0.05 }) as u32;
        let video = VideoSettingsBuilder::new(w, h)
            .sub_type(VideoSettingsSubType::H264)
            .frame_rate(opts.fps)
            .bitrate(bits.clamp(2_000_000, 40_000_000));

        let clock = Arc::new(Clock::default());
        let has_audio = opts.audio.mic.is_some() || opts.audio.system;
        let encoder: Shared = Arc::new(Mutex::new(Some(VideoEncoder::new(
            video,
            AudioSettingsBuilder::new().disabled(!has_audio),
            ContainerSettingsBuilder::new(),
            &opts.path,
        )?)));

        let audio = has_audio
            .then(|| {
                let enc = encoder.clone();
                audio::Pipe::start(opts.audio, clock.clone(), level, move |pcm| {
                    if let Some(e) = enc.lock().unwrap().as_mut() {
                        let _ = e.send_audio_buffer(pcm, 0);
                    }
                })
            })
            .flatten();

        let camera = opts.camera.map(|c| (camera::Camera::open(&c.device), c));
        let (camera, cam_feed) = match camera {
            Some((cam, c)) => {
                let feed = cam.latest.clone();
                (Some(cam), Some((feed, c)))
            }
            None => (None, None),
        };

        let frames = Arc::new(AtomicU64::new(0));
        let latest: Latest = Arc::new(Mutex::new(None));
        let compositor = Compositor::start(encoder.clone(), latest.clone(), cam_feed, (w, h), opts.fps, clock.clone(), ended.clone());

        let flags = Flags { latest, frames: frames.clone(), size: (w, h), crop, ended: ended.clone() };
        let cursor = if opts.cursor { CursorCaptureSettings::WithCursor } else { CursorCaptureSettings::WithoutCursor };
        let interval = MinimumUpdateIntervalSettings::Custom(Duration::from_micros(1_000_000 / opts.fps as u64));
        macro_rules! settings {
            ($item:expr) => {
                Settings::new(
                    $item,
                    cursor,
                    DrawBorderSettings::WithoutBorder,
                    SecondaryWindowSettings::Default,
                    interval,
                    DirtyRegionSettings::Default,
                    ColorFormat::Bgra8,
                    flags,
                )
            };
        }
        let control = match opts.source {
            Source::Window(hwnd) => Handler::start_free_threaded(settings!(Window::from_raw_hwnd(hwnd as _))),
            Source::Display(m, _) => Handler::start_free_threaded(settings!(Monitor::from_raw_hmonitor(m as _))),
        };

        let rec = Self {
            control: None,
            audio,
            compositor: Some(compositor),
            camera,
            encoder,
            ended,
            clock,
            frames,
            path: opts.path,
        };
        match control {
            Ok(c) => Ok(Self { control: Some(c), ..rec }),
            Err(e) => {
                let path = rec.path.clone();
                let _ = rec.finish();
                let _ = std::fs::remove_file(path);
                Err(format!("{e:?}").into())
            }
        }
    }

    /// Stops capture and audio, then finalizes the MP4. Blocks until the file is complete.
    pub fn finish(mut self) -> Result<PathBuf, Error> {
        self.ended.stopping.store(true, Ordering::Relaxed);
        if let Some(c) = self.control.take() {
            let _ = c.stop();
        }
        if let Some(a) = self.audio.take() {
            a.stop();
        }
        // Its last frame holds the final image up to the moment Stop was pressed.
        if let Some(c) = self.compositor.take() {
            c.stop();
        }
        if let Some(c) = self.camera.take() {
            c.close();
        }
        let enc = self.encoder.lock().unwrap().take();
        if let Some(e) = enc {
            e.finish()?;
        }
        Ok(self.path)
    }
}
