//! Settings as `key=value` lines in %APPDATA%\Lumen-rs\settings.txt (no serde needed).

use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Window,
    Display,
    Region,
}

#[derive(Clone)]
pub struct Settings {
    pub mode: Mode,
    pub display: usize,
    /// x, y, w, h inside `display`, physical pixels.
    pub region: Option<[u32; 4]>,
    pub camera_on: bool,
    pub camera: Option<String>,
    /// 0 top-left, 1 top-right, 2 bottom-left, 3 bottom-right.
    pub corner: usize,
    /// 0 S, 1 M, 2 L.
    pub camera_size: usize,
    pub mic: Option<String>,
    pub gain: f32,
    pub denoise: bool,
    pub system: bool,
    pub fps60: bool,
    pub high: bool,
    pub cursor: bool,
    pub countdown: bool,
    pub out_dir: PathBuf,
}

fn file() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Lumen-rs").join("settings.txt"))
}

impl Default for Settings {
    fn default() -> Self {
        let videos = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default().join("Videos").join("Lumen");
        Self {
            mode: Mode::Window,
            display: 0,
            region: None,
            camera_on: false,
            camera: None,
            corner: 3,
            camera_size: 1,
            mic: None,
            gain: 1.0,
            denoise: true,
            system: false,
            fps60: false,
            high: true,
            cursor: true,
            countdown: true,
            out_dir: videos,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Self::default();
        let Some(text) = file().and_then(|f| std::fs::read_to_string(f).ok()) else { return s };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let on = v == "1";
            match k {
                "mode" => {
                    s.mode = match v {
                        "display" => Mode::Display,
                        "region" => Mode::Region,
                        _ => Mode::Window,
                    }
                }
                "display" => s.display = v.parse().unwrap_or(0),
                "region" => {
                    let n: Vec<u32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                    s.region = (n.len() == 4 && n[2] >= 64 && n[3] >= 64).then(|| [n[0], n[1], n[2], n[3]]);
                }
                "camera_on" => s.camera_on = on,
                "camera" if !v.is_empty() => s.camera = Some(v.to_string()),
                "corner" => s.corner = v.parse::<usize>().unwrap_or(3).min(3),
                "camera_size" => s.camera_size = v.parse::<usize>().unwrap_or(1).min(2),
                "mic" if !v.is_empty() => s.mic = Some(v.to_string()),
                "gain" => s.gain = v.parse::<f32>().unwrap_or(1.0).clamp(0.5, 2.0),
                "denoise" => s.denoise = on,
                "system" => s.system = on,
                "fps60" => s.fps60 = on,
                "high" => s.high = on,
                "cursor" => s.cursor = on,
                "countdown" => s.countdown = on,
                "out" if !v.is_empty() => s.out_dir = PathBuf::from(v),
                _ => {}
            }
        }
        s
    }

    pub fn save(&self) {
        let Some(f) = file() else { return };
        let b = |x: bool| if x { "1" } else { "0" };
        let mode = match self.mode {
            Mode::Window => "window",
            Mode::Display => "display",
            Mode::Region => "region",
        };
        let region = self.region.map(|r| format!("{},{},{},{}", r[0], r[1], r[2], r[3])).unwrap_or_default();
        let text = format!(
            "mode={mode}\ndisplay={}\nregion={region}\ncamera_on={}\ncamera={}\ncorner={}\ncamera_size={}\n\
             mic={}\ngain={}\ndenoise={}\nsystem={}\nfps60={}\nhigh={}\ncursor={}\ncountdown={}\nout={}\n",
            self.display,
            b(self.camera_on),
            self.camera.as_deref().unwrap_or(""),
            self.corner,
            self.camera_size,
            self.mic.as_deref().unwrap_or(""),
            self.gain,
            b(self.denoise),
            b(self.system),
            b(self.fps60),
            b(self.high),
            b(self.cursor),
            b(self.countdown),
            self.out_dir.display()
        );
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = f.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(tmp, f);
        }
    }
}
