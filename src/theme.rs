//! Colours: the NIRUX theme in use, or a retro one.
//!
//! `nirux` follows the desktop: the theme named in
//! `~/.config/niruwm/config.toml` (a file of that name in
//! `~/.config/niruwm/themes/` wins over the built-in one) with the
//! overrides of `appearance.toml`. The others are fixed palettes in the
//! spirit of old terminals, and `native` keeps the terminal's own colours.
use ratatui::style::Color;
use std::path::PathBuf;

pub const NAMES: [&str; 6] = ["nirux", "amber", "phosphor", "c64", "paper", "native"];

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub name: String,
    pub bg: Color,
    pub surface: Color,
    pub fg: Color,
    pub muted: Color,
    pub border: Color,
    pub accent: Color,
    pub select_bg: Color,
    pub select_fg: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
}

pub type Rgb = (u8, u8, u8);

fn hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

fn luma(c: Rgb) -> f32 {
    0.2126 * f32::from(c.0) + 0.7152 * f32::from(c.1) + 0.0722 * f32::from(c.2)
}

/// The five colours a NIRU theme file holds, plus overrides.
#[derive(Clone, Debug, Default)]
pub struct Base {
    pub name: String,
    pub background: Option<Rgb>,
    pub foreground: Option<Rgb>,
    pub muted: Option<Rgb>,
    pub border: Option<Rgb>,
    pub focus: Option<Rgb>,
    pub surface: Option<Rgb>,
    pub accent: Option<Rgb>,
    pub selected: Option<Rgb>,
    pub success: Option<Rgb>,
    pub warning: Option<Rgb>,
    pub error: Option<Rgb>,
}

impl Base {
    fn read(&mut self, text: &str, table: Option<&str>) {
        let Ok(v) = text.parse::<toml::Value>() else {
            return;
        };
        let t = match table {
            Some(name) => match v.get(name) {
                Some(t) => t.clone(),
                None => return,
            },
            None => v,
        };
        let get = |k: &str| t.get(k).and_then(|x| x.as_str()).and_then(hex);
        if let Some(n) = t.get("name").and_then(|x| x.as_str()) {
            self.name = n.to_string();
        }
        for (slot, key) in [
            (&mut self.background, "background"),
            (&mut self.foreground, "foreground"),
            (&mut self.muted, "muted"),
            (&mut self.border, "border"),
            (&mut self.focus, "focus"),
            (&mut self.surface, "surface"),
            (&mut self.accent, "accent"),
            (&mut self.selected, "selected"),
            (&mut self.success, "success"),
            (&mut self.warning, "warning"),
            (&mut self.error, "error"),
        ] {
            if let Some(c) = get(key) {
                *slot = Some(c);
            }
        }
    }

    pub fn palette(&self, name: &str) -> Palette {
        let bg = self.background.unwrap_or((0x0f, 0x0f, 0x10));
        let fg = self.foreground.unwrap_or((0xe5, 0xe5, 0xe5));
        let dark = luma(bg) < 128.0;
        let focus = self.focus.unwrap_or((0x5a, 0x5a, 0x60));
        // A focus colour as dark as the ground cannot mark a selection: lift it.
        let accent = self.accent.unwrap_or_else(|| {
            if (luma(focus) - luma(bg)).abs() < 100.0 {
                mix(focus, fg, 0.55)
            } else {
                focus
            }
        });
        Palette {
            name: if self.name.is_empty() {
                name.to_string()
            } else {
                self.name.clone()
            },
            bg: rgb(bg),
            surface: rgb(self.surface.unwrap_or(mix(bg, fg, 0.05))),
            fg: rgb(fg),
            muted: rgb(self.muted.unwrap_or(mix(bg, fg, 0.6))),
            border: rgb(self.border.unwrap_or(mix(bg, fg, 0.18))),
            accent: rgb(accent),
            select_bg: rgb(self.selected.unwrap_or(mix(bg, accent, 0.35))),
            select_fg: rgb(fg),
            ok: rgb(self.success.unwrap_or(if dark {
                (0x8f, 0xbf, 0x7f)
            } else {
                (0x3a, 0x7a, 0x2a)
            })),
            warn: rgb(self.warning.unwrap_or(if dark {
                (0xd7, 0xb3, 0x6a)
            } else {
                (0x9a, 0x6a, 0x00)
            })),
            err: rgb(self.error.unwrap_or(if dark {
                (0xd7, 0x7a, 0x7a)
            } else {
                (0xa0, 0x20, 0x20)
            })),
        }
    }
}

fn builtin_niru(name: &str) -> Option<&'static str> {
    Some(match name {
        "niru-noir" => include_str!("../themes/niru-noir.toml"),
        "niru-nord" => include_str!("../themes/niru-nord.toml"),
        "niru-satie" => include_str!("../themes/niru-satie.toml"),
        "niru-carl-larsson" => include_str!("../themes/niru-carl-larsson.toml"),
        "niru-forest-vigil" => include_str!("../themes/niru-forest-vigil.toml"),
        "niru-blood-of-urdr" => include_str!("../themes/niru-blood-of-urdr.toml"),
        "niru-root-and-stone" => include_str!("../themes/niru-root-and-stone.toml"),
        "niru-blue-horizon" => include_str!("../themes/niru-blue-horizon.toml"),
        "niru-the-norns" => include_str!("../themes/niru-the-norns.toml"),
        "niru-winter-nights" => include_str!("../themes/niru-winter-nights.toml"),
        _ => return None,
    })
}

fn niruwm_dir() -> PathBuf {
    crate::config::config_home().join("niruwm")
}

/// The desktop's theme, as NIRUBAR draws it.
pub fn nirux_base() -> Base {
    let dir = niruwm_dir();
    let mut base = Base::default();
    let name = std::fs::read_to_string(dir.join("config.toml"))
        .ok()
        .and_then(|t| t.parse::<toml::Value>().ok())
        .and_then(|v| v.get("theme").and_then(|x| x.as_str()).map(str::to_string))
        .filter(|n| {
            n.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .unwrap_or_else(|| "niru-noir".into());
    let user = dir.join("themes").join(format!("{name}.toml"));
    match std::fs::read_to_string(&user) {
        Ok(text) => base.read(&text, None),
        Err(_) => base.read(
            builtin_niru(&name)
                .or(builtin_niru("niru-noir"))
                .unwrap_or(""),
            None,
        ),
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("appearance.toml")) {
        base.read(&text, Some("colors"));
    }
    base
}

fn fixed(name: &str, c: [Rgb; 6]) -> Palette {
    // ground, text, dim, accent, ok, warn
    let [bg, fg, muted, accent, ok, warn] = c;
    Palette {
        name: name.into(),
        bg: rgb(bg),
        surface: rgb(mix(bg, fg, 0.06)),
        fg: rgb(fg),
        muted: rgb(muted),
        border: rgb(mix(bg, fg, 0.3)),
        accent: rgb(accent),
        select_bg: rgb(mix(bg, accent, 0.4)),
        select_fg: rgb(if luma(bg) < 128.0 {
            (255, 255, 255)
        } else {
            (0, 0, 0)
        }),
        ok: rgb(ok),
        warn: rgb(warn),
        err: rgb(if luma(bg) < 128.0 {
            (255, 96, 80)
        } else {
            (170, 30, 30)
        }),
    }
}

pub fn palette(name: &str) -> Palette {
    match name {
        "amber" => fixed(
            "Amber",
            [
                (18, 10, 2),
                (255, 176, 0),
                (178, 110, 0),
                (255, 214, 102),
                (255, 200, 60),
                (255, 140, 0),
            ],
        ),
        "phosphor" => fixed(
            "Phosphor",
            [
                (4, 12, 4),
                (51, 255, 102),
                (24, 150, 60),
                (180, 255, 190),
                (120, 255, 140),
                (220, 255, 80),
            ],
        ),
        "c64" => fixed(
            "C64",
            [
                (64, 49, 141),
                (160, 154, 255),
                (120, 105, 196),
                (255, 255, 255),
                (152, 230, 141),
                (237, 240, 113),
            ],
        ),
        "paper" => fixed(
            "Paper",
            [
                (238, 232, 213),
                (40, 36, 30),
                (110, 100, 86),
                (135, 75, 20),
                (60, 110, 40),
                (150, 100, 0),
            ],
        ),
        "native" => Palette {
            name: "Terminal".into(),
            bg: Color::Reset,
            surface: Color::Reset,
            fg: Color::Reset,
            muted: Color::DarkGray,
            border: Color::DarkGray,
            accent: Color::Cyan,
            select_bg: Color::Blue,
            select_fg: Color::White,
            ok: Color::Green,
            warn: Color::Yellow,
            err: Color::Red,
        },
        _ => nirux_base().palette("NIRUX"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_niru_theme_and_overrides_make_a_palette() {
        let mut b = Base::default();
        b.read(include_str!("../themes/niru-nord.toml"), None);
        b.read("[colors]\nerror = \"ff0000\"\n", Some("colors"));
        let p = b.palette("x");
        assert_eq!(p.name, "Niru Nord");
        assert_eq!(p.bg, Color::Rgb(0x2e, 0x34, 0x40));
        assert_eq!(p.err, Color::Rgb(255, 0, 0));
        // Noir's focus is close to its ground: the accent is lifted.
        let mut n = Base::default();
        n.read(include_str!("../themes/niru-noir.toml"), None);
        let p = n.palette("x");
        assert_ne!(p.accent, Color::Rgb(0x5a, 0x5a, 0x60));
        for name in NAMES {
            let _ = palette(name);
        }
    }
}
