use crate::tmplayer::data::assets;
use crate::tmplayer::ui::theme::{Theme, ThemeName, ThemePalette, detect_color_capability};
use anyhow::Result;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

pub struct ThemeLoader;

#[derive(Debug, Deserialize)]
struct ThemeToml {
    text: String,
    subtext: String,
    base: String,
    surface: String,
    buff: Option<String>,
    accent: String,
    accent2: String,
    accent3: String,
}

impl ThemeLoader {
    pub fn load(name: &str) -> Result<Theme> {
        let _ = assets::ensure_assets_ready();
        let name = ThemeName::from_str_or_system(name);

        let rel = match name {
            ThemeName::System => PathBuf::from("themes/system.toml"),
            ThemeName::Latte => PathBuf::from("themes/catppuccin_latte.toml"),
            ThemeName::Frappe => PathBuf::from("themes/catppuccin_frappe.toml"),
            ThemeName::Macchiato => PathBuf::from("themes/catppuccin_macchiato.toml"),
            ThemeName::Mocha => PathBuf::from("themes/catppuccin_mocha.toml"),
            ThemeName::AtomOneDark => PathBuf::from("themes/atom_one_dark.toml"),
            ThemeName::AtomOneLight => PathBuf::from("themes/atom_one_light.toml"),
        };

        let path = assets::resolve_asset_path(&rel);
        let raw = fs::read_to_string(&path)?;
        let t: ThemeToml = toml::from_str(&raw)?;
        let buff_hex = if let Some(buff) = t.buff.clone() {
            buff
        } else {
            let generated = derive_buff_hex(&t.surface);
            let upgraded = inject_buff_entry(&raw, &generated);
            let _ = fs::write(&path, upgraded);
            generated
        };
        let capability = detect_color_capability();
        Ok(Theme {
            name,
            palette: ThemePalette {
                text: parse_hex(&t.text),
                subtext: parse_hex(&t.subtext),
                base: parse_hex(&t.base),
                surface: parse_hex(&t.surface),
                buff: parse_hex(&buff_hex),
                accent: parse_hex(&t.accent),
                accent2: parse_hex(&t.accent2),
                accent3: parse_hex(&t.accent3),
            },
            capability,
        })
    }
}

fn parse_hex(s: &str) -> (u8, u8, u8) {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return (255, 255, 255);
    }

    let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(255);
    let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(255);
    let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(255);
    (r, g, b)
}

fn derive_buff_hex(surface_hex: &str) -> String {
    let (r, g, b) = parse_hex(surface_hex);
    format!(
        "#{:02X}{:02X}{:02X}",
        r.saturating_add(10),
        g.saturating_add(10),
        b.saturating_add(10)
    )
}

fn inject_buff_entry(raw: &str, buff_hex: &str) -> String {
    if raw.lines().any(|line| is_toml_key(line, "buff")) {
        return raw.to_string();
    }

    let mut out = String::with_capacity(raw.len() + 24);
    let mut inserted = false;

    for line in raw.lines() {
        out.push_str(line);
        out.push('\n');
        if !inserted && is_toml_key(line, "surface") {
            out.push_str(&format!("buff = \"{}\"\n", buff_hex));
            inserted = true;
        }
    }

    if !inserted {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("buff = \"{}\"\n", buff_hex));
    }

    out
}

fn is_toml_key(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with(key) {
        return false;
    }
    trimmed[key.len()..].trim_start().starts_with('=')
}
