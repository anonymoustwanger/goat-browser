//! Colour themes (the same palettes as Goat Browser). Used by the native toolbar and, as CSS
//! variables, by the built-in pages.

use egui::Color32;

pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub bg: &'static str,
    pub bg2: &'static str,
    pub bg3: &'static str,
    pub line: &'static str,
    pub text: &'static str,
    pub muted: &'static str,
    pub accent: &'static str,
    pub accent2: &'static str,
    pub accent_ink: &'static str,
    pub warn: &'static str,
    pub danger: &'static str,
}

macro_rules! theme {
    ($id:expr, $name:expr, $dark:expr, $a2:expr, $bg:expr, $bg2:expr, $bg3:expr, $line:expr, $text:expr, $muted:expr, $acc:expr, $ink:expr, $warn:expr, $danger:expr) => {
        Theme { id: $id, name: $name, dark: $dark, accent2: $a2, bg: $bg, bg2: $bg2, bg3: $bg3, line: $line, text: $text,
                muted: $muted, accent: $acc, accent_ink: $ink, warn: $warn, danger: $danger }
    };
}

pub const THEMES: &[Theme] = &[
    theme!("midnight", "Midnight", true, "#6ea8ff", "#101214", "#181b1f", "#22262b", "#2b3036", "#e7e9ec", "#8b939c", "#5fd3b3", "#06231b", "#f0b35a", "#ff7a66"),
    theme!("daylight", "Daylight", false, "#4a6cf7", "#eef0f3", "#ffffff", "#e2e5ea", "#d4d9e0", "#16191d", "#59616c", "#0d8a6c", "#ffffff", "#a35f00", "#c93a2b"),
    theme!("forest", "Forest", true, "#d8c35b", "#0f1511", "#161e18", "#1f2a22", "#2a372d", "#e4ece5", "#8fa394", "#8bd17c", "#0c1f0a", "#e8c15a", "#ff806b"),
    theme!("ocean", "Ocean", true, "#8a7dff", "#0b1220", "#111a2c", "#18243a", "#22304a", "#e3eaf5", "#8696b0", "#5ab0ff", "#04182e", "#f0b35a", "#ff7a7a"),
    theme!("sunset", "Sunset", true, "#ff5e8a", "#1a1012", "#24161a", "#301d22", "#3d262c", "#f3e6e3", "#b1938f", "#ff9e6b", "#2b1206", "#ffd166", "#ff6b81"),
    theme!("grape", "Grape", true, "#ff7ac6", "#140f1c", "#1c1527", "#261d34", "#322742", "#ece6f5", "#9c8fb0", "#c39bff", "#1d0b38", "#f5c26b", "#ff7aa8"),
    theme!("mono", "Mono", true, "#9a9a9a", "#0c0c0c", "#161616", "#222222", "#2e2e2e", "#f2f2f2", "#8f8f8f", "#f2f2f2", "#0c0c0c", "#d9d9d9", "#ff6b6b"),
    theme!("paper", "Paper", false, "#d4a03c", "#f4efe6", "#fffaf2", "#eae2d4", "#dcd1bf", "#2a241c", "#6f6353", "#b85a27", "#ffffff", "#9a6400", "#b8322a"),
];

/// "system" follows Windows light/dark (Daylight / Midnight). Unknown ids fall back to Midnight.
pub fn get(id: &str, system_dark: bool) -> &'static Theme {
    let id = if id == "system" { if system_dark { "midnight" } else { "daylight" } } else { id };
    if let Some(t) = THEMES.iter().find(|t| t.id == id) {
        return t;
    }
    CUSTOM.lock().ok().and_then(|m| m.iter().find(|t| t.id == id).copied()).unwrap_or(&THEMES[0])
}

/// Your own themes, built from five colours like Goat's editor (the rest are worked out).
static CUSTOM: std::sync::Mutex<Vec<&'static Theme>> = std::sync::Mutex::new(Vec::new());

fn rgb(s: &str) -> (f32, f32, f32) {
    let c = hex(s);
    (c.r() as f32, c.g() as f32, c.b() as f32)
}
fn mix_hex(a: &str, b: &str, t: f32) -> String {
    let (a, b) = (rgb(a), rgb(b));
    let m = |x: f32, y: f32| (x + (y - x) * t).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}
pub fn luminance(s: &str) -> f32 {
    let (r, g, b) = rgb(s);
    let f = |c: f32| {
        let c = c / 255.0;
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Register the custom themes (call when settings change). Unchanged themes are reused.
pub fn set_custom(list: &[crate::store::CustomTheme]) {
    let Ok(mut m) = CUSTOM.lock() else { return };
    for c in list {
        let fresh = |t: &&&'static Theme| t.id == c.id && t.name == c.name && t.bg == c.bg && t.bg2 == c.surface && t.text == c.text && t.accent == c.accent && t.accent2 == c.accent2;
        if m.iter().any(|t| fresh(&t)) {
            continue;
        }
        m.retain(|t| t.id != c.id);
        let dark = luminance(&c.bg) < 0.4;
        let t: &'static Theme = Box::leak(Box::new(Theme {
            id: leak(c.id.clone()),
            name: leak(c.name.clone()),
            dark,
            bg: leak(c.bg.clone()),
            bg2: leak(c.surface.clone()),
            bg3: leak(mix_hex(&c.surface, &c.text, 0.08)),
            line: leak(mix_hex(&c.surface, &c.text, 0.15)),
            text: leak(c.text.clone()),
            muted: leak(mix_hex(&c.text, &c.bg, 0.42)),
            accent: leak(c.accent.clone()),
            accent2: leak(c.accent2.clone()),
            accent_ink: if luminance(&c.accent) > 0.4 { "#0b0d10" } else { "#ffffff" },
            warn: if dark { "#f0b35a" } else { "#a35f00" },
            danger: if dark { "#ff7a66" } else { "#c93a2b" },
        }));
        m.push(t);
    }
    m.retain(|t| list.iter().any(|c| c.id == t.id));
}

/// Built-in themes followed by your own.
pub fn all() -> Vec<&'static Theme> {
    let mut v: Vec<&'static Theme> = THEMES.iter().collect();
    if let Ok(m) = CUSTOM.lock() {
        v.extend(m.iter().copied());
    }
    v
}

pub fn hex(s: &str) -> Color32 {
    let h = s.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Resolved colours for painting.
#[derive(Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub bg2: Color32,
    pub bg3: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent2: Color32,
    pub ink: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub gradients: bool,
}

impl Theme {
    pub fn palette(&self, gradients: bool) -> Palette {
        Palette {
            dark: self.dark,
            bg: hex(self.bg),
            bg2: hex(self.bg2),
            bg3: hex(self.bg3),
            line: hex(self.line),
            text: hex(self.text),
            muted: hex(self.muted),
            accent: hex(self.accent),
            accent2: hex(self.accent2),
            ink: hex(self.accent_ink),
            warn: hex(self.warn),
            danger: hex(self.danger),
            gradients,
        }
    }

    /// CSS custom properties for the built-in pages (same names as Goat's themes.css).
    pub fn css_vars(&self, gradients: bool) -> String {
        let mut s = format!(
            "--bg:{};--bg-2:{};--bg-3:{};--line:{};--text:{};--muted:{};--accent:{};--accent-2:{};--accent-ink:{};--warn:{};--danger:{};color-scheme:{};",
            self.bg, self.bg2, self.bg3, self.line, self.text, self.muted, self.accent, self.accent2, self.accent_ink,
            self.warn, self.danger, if self.dark { "dark" } else { "light" }
        );
        if gradients {
            s.push_str(
                "--grad-accent:linear-gradient(135deg,var(--accent),var(--accent-2));\
                 --grad-hero:linear-gradient(135deg,color-mix(in srgb,var(--accent) 18%,var(--bg-2)),var(--bg-2) 60%,color-mix(in srgb,var(--accent-2) 12%,var(--bg-2)));\
                 --grad-page:radial-gradient(900px 520px at 8% -12%,color-mix(in srgb,var(--accent) 16%,transparent),transparent 70%),\
                 radial-gradient(800px 520px at 100% 110%,color-mix(in srgb,var(--accent-2) 14%,transparent),transparent 70%),var(--bg);",
            );
        } else {
            s.push_str("--grad-accent:var(--accent);--grad-hero:var(--bg-2);--grad-page:var(--bg);");
        }
        s
    }
}

impl Palette {
    /// Blend towards `to` (t = 0 → self, 1 → to). Used to cross-fade theme changes.
    pub fn lerp(&self, to: &Palette, t: f32) -> Palette {
        let m = |a: Color32, b: Color32| crate::paint::mix(b, a, t);
        Palette {
            dark: if t < 0.5 { self.dark } else { to.dark },
            bg: m(self.bg, to.bg),
            bg2: m(self.bg2, to.bg2),
            bg3: m(self.bg3, to.bg3),
            line: m(self.line, to.line),
            text: m(self.text, to.text),
            muted: m(self.muted, to.muted),
            accent: m(self.accent, to.accent),
            accent2: m(self.accent2, to.accent2),
            ink: m(self.ink, to.ink),
            warn: m(self.warn, to.warn),
            danger: m(self.danger, to.danger),
            gradients: to.gradients,
        }
    }
}
