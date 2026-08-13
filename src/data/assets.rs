use crate::STORAGE;
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};

const ENV_ASSET_DIR: &str = "CNMPLAYER_ASSET_DIR";

const DEFAULT_CONFIG_TOML: &str = include_str!("../../config/default.toml");

const THEME_SYSTEM_TOML: &str = include_str!("../../themes/system.toml");
const THEME_LATTE_TOML: &str = include_str!("../../themes/catppuccin_latte.toml");
const THEME_FRAPPE_TOML: &str = include_str!("../../themes/catppuccin_frappe.toml");
const THEME_MACCHIATO_TOML: &str = include_str!("../../themes/catppuccin_macchiato.toml");
const THEME_MOCHA_TOML: &str = include_str!("../../themes/catppuccin_mocha.toml");

pub fn resolve_asset_root() -> Cow<'static, PathBuf> {
    if let Some(path) = std::env::var_os(ENV_ASSET_DIR) {
        return Cow::Owned(PathBuf::from(path));
    }

    let _ = ensure_all_assets(&STORAGE.config);
    Cow::Borrowed(&STORAGE.config)
}

pub fn resolve_asset_path(rel: &Path) -> PathBuf {
    resolve_asset_root().join(rel)
}

pub fn resolve_config_path() -> PathBuf {
    resolve_asset_path(Path::new("config/default.toml"))
}

pub fn ensure_assets_ready() -> Result<&'static PathBuf> {
    ensure_all_assets(&STORAGE.config)?;
    Ok(&STORAGE.config)
}

fn ensure_all_assets(root: &Path) -> Result<()> {
    ensure_dir(&root.join("config"))?;
    ensure_dir(&root.join("themes"))?;

    write_if_missing(&root.join("config/default.toml"), DEFAULT_CONFIG_TOML)?;
    ensure_themes(root)?;

    Ok(())
}

fn ensure_themes(root: &Path) -> Result<()> {
    ensure_dir(&root.join("themes"))?;

    write_if_missing(&root.join("themes/system.toml"), THEME_SYSTEM_TOML)?;
    write_if_missing(&root.join("themes/catppuccin_latte.toml"), THEME_LATTE_TOML)?;
    write_if_missing(
        &root.join("themes/catppuccin_frappe.toml"),
        THEME_FRAPPE_TOML,
    )?;
    write_if_missing(
        &root.join("themes/catppuccin_macchiato.toml"),
        THEME_MACCHIATO_TOML,
    )?;
    write_if_missing(&root.join("themes/catppuccin_mocha.toml"), THEME_MOCHA_TOML)?;

    Ok(())
}

fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("mkdir {}", path.display()))
}

fn write_if_missing(path: &Path, contents: &str) -> Result<()> {
    if path.is_file() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}
