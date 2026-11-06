//! The tray icon, drawn per pixel at the size the notification area uses (no scaling blur): the
//! fuller of the two windows as a number on top, and two bars below it, session over week.
//!
//! The digits are a 3×5 pixel font scaled by whole pixels; colour is the taskbar's text colour
//! until a window passes 60 % (amber) or 85 % (red). Bars use the same colours on a faint track.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    /// The number: the fuller window, 0..=100+. `None`: no reading yet ("--").
    pub headline: Option<f64>,
    pub session: Option<f64>,
    pub week: Option<f64>,
    /// The taskbar is light (dark ink), else dark (white ink).
    pub light_taskbar: bool,
    /// The saved login needs `/login` (or there is no live account): dimmed.
    pub dim: bool,
}

type Rgb = [u8; 3];

const WARN_AT: f64 = 60.0;
const DANGER_AT: f64 = 85.0;

fn ink(light: bool) -> Rgb {
    if light { [0x1F, 0x23, 0x28] } else { [0xFF, 0xFF, 0xFF] }
}

fn tone(percent: f64, light: bool) -> Option<Rgb> {
    match (percent >= DANGER_AT, percent >= WARN_AT, light) {
        (true, _, false) => Some([0xFF, 0x5F, 0x56]),
        (true, _, true) => Some([0xCF, 0x22, 0x2E]),
        (false, true, false) => Some([0xF5, 0xB7, 0x32]),
        (false, true, true) => Some([0xB3, 0x6B, 0x00]),
        _ => None,
    }
}

fn bar_fill(percent: f64, light: bool) -> Rgb {
    tone(percent, light).unwrap_or(if light { [0x1A, 0x7F, 0x37] } else { [0x4A, 0xDE, 0x80] })
}

/// 3×5 glyphs, rows top to bottom, bit 2 = left column.
fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        '!' => [0b010, 0b010, 0b010, 0b000, 0b010],
        _ => [0; 5],
    }
}

struct Canvas {
    size: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn new(size: usize) -> Self {
        Self {
            size,
            px: vec![0; size * size * 4],
        }
    }

    fn rect(&mut self, x0: usize, y0: usize, w: usize, h: usize, rgb: Rgb, alpha: u8) {
        for y in y0..(y0 + h).min(self.size) {
            for x in x0..(x0 + w).min(self.size) {
                let o = (y * self.size + x) * 4;
                self.px[o..o + 3].copy_from_slice(&rgb);
                self.px[o + 3] = alpha;
            }
        }
    }
}

/// RGBA pixels, `size`×`size`.
pub fn render(face: &Face, size: u32) -> Vec<u8> {
    let size = size.clamp(16, 64) as usize;
    let mut c = Canvas::new(size);
    let light = face.light_taskbar;
    let alpha = if face.dim { 140 } else { 255 };

    // The number.
    let k = (size / 8).max(1);
    let text = match face.headline {
        None => "--".to_owned(),
        Some(p) if p >= 100.0 => "!".to_owned(),
        Some(p) => format!("{}", p.round().clamp(0.0, 99.0) as u32),
    };
    let color = face
        .headline
        .and_then(|p| tone(p, light))
        .unwrap_or_else(|| ink(light));
    let glyphs: Vec<[u8; 5]> = text.chars().map(glyph).collect();
    let width = glyphs.len() * 3 * k + glyphs.len().saturating_sub(1) * k;
    let mut x = size.saturating_sub(width) / 2;
    for g in &glyphs {
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    c.rect(x + col * k, row * k, k, k, color, alpha);
                }
            }
        }
        x += 4 * k;
    }

    // The bars, session over week, anchored to the bottom edge; any spare row goes above them.
    let rest = size - 5 * k;
    let gap = (rest / 6).max(1);
    let thick = ((rest - 2 * gap) / 2).max(1);
    let (x0, span) = (1, size - 2);
    for (i, value) in [face.session, face.week].into_iter().enumerate() {
        let y = size - (2 - i) * thick - (1 - i) * gap;
        c.rect(x0, y, span, thick, ink(light), if face.dim { 40 } else { 70 });
        if let Some(p) = value {
            let filled = ((p.clamp(0.0, 100.0) / 100.0) * span as f64).round() as usize;
            let filled = if p > 0.0 { filled.max(1) } else { 0 };
            c.rect(x0, y, filled, thick, bar_fill(p, light), alpha);
        }
    }
    c.px
}

/// The notification area's icon size in pixels (16 at 100 % scaling, 24 at 150 %, ...).
#[cfg(windows)]
pub fn tray_size() -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
    // SAFETY: a plain query with no pointers.
    let px = unsafe { GetSystemMetrics(SM_CXSMICON) };
    u32::try_from(px).unwrap_or(16).clamp(16, 64)
}

#[cfg(not(windows))]
pub fn tray_size() -> u32 {
    32
}

/// Whether the taskbar uses the light theme (`SystemUsesLightTheme`).
#[cfg(windows)]
pub fn light_taskbar() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut value: u32 = 0;
    let mut len = u32::try_from(std::mem::size_of::<u32>()).unwrap_or(4);
    // SAFETY: `value` and `len` outlive the call; the buffer is one DWORD as declared.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut len),
        )
    };
    status.is_ok() && value == 1
}

#[cfg(not(windows))]
pub fn light_taskbar() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(px: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        let o = (y * size + x) * 4;
        [px[o], px[o + 1], px[o + 2], px[o + 3]]
    }

    #[test]
    fn layout_fits_every_tray_size() {
        for size in [16u32, 20, 24, 32, 40, 48] {
            let face = Face {
                headline: Some(88.0),
                session: Some(88.0),
                week: Some(20.0),
                light_taskbar: false,
                dim: false,
            };
            let px = render(&face, size);
            assert_eq!(px.len(), (size * size * 4) as usize);
            let s = size as usize;
            let k = (s / 8).max(1);
            let painted = |y: usize| (0..s).any(|x| at(&px, s, x, y)[3] > 0);
            assert!(painted(s - 1), "size {size}: the week bar sits on the bottom edge");
            assert!(painted(5 * k - 1), "size {size}: the digits' last row");
            assert!(!painted(5 * k), "size {size}: a gap between the digits and the bars");
        }
    }

    #[test]
    fn colours_follow_the_fuller_window() {
        let s = 16usize;
        let face = Face {
            headline: Some(90.0),
            session: Some(90.0),
            week: Some(10.0),
            light_taskbar: false,
            dim: false,
        };
        let px = render(&face, 16);
        // "90": the first digit's top-left pixel (x=1, y=0 at k=2) is red.
        assert_eq!(at(&px, s, 1, 0), [0xFF, 0x5F, 0x56, 255]);
        // Session bar (row 11) filled red at its left end; week bar (row 14) green at its left end.
        assert_eq!(at(&px, s, 1, 11)[..3], [0xFF, 0x5F, 0x56]);
        assert_eq!(at(&px, s, 1, 14)[..3], [0x4A, 0xDE, 0x80]);
        // The week bar's right end is the faint track.
        assert_eq!(at(&px, s, 14, 14)[3], 70);
    }
}

/// `cargo test icon_previews -- --ignored`: raw RGBA renders in `target/icon-previews` (for a look).
#[cfg(test)]
mod previews {
    #[test]
    #[ignore]
    fn icon_previews() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/icon-previews");
        std::fs::create_dir_all(&dir).unwrap();
        let cases = [
            ("27-54", Some(54.0), Some(27.0), Some(54.0)),
            ("80-20", Some(80.0), Some(80.0), Some(20.0)),
            ("92-61", Some(92.0), Some(92.0), Some(61.0)),
            ("5-3", Some(5.0), Some(5.0), Some(3.0)),
            ("100", Some(100.0), Some(100.0), Some(40.0)),
            ("none", None, None, None),
        ];
        for light in [false, true] {
            for size in [16u32, 24, 32] {
                for (name, headline, session, week) in cases {
                    let face = super::Face { headline, session, week, light_taskbar: light, dim: headline.is_none() };
                    let px = super::render(&face, size);
                    let theme = if light { "light" } else { "dark" };
                    std::fs::write(dir.join(format!("{theme}-{size}-{name}.rgba")), px).unwrap();
                }
            }
        }
    }
}
