use crate::STORAGE;
use crate::data::atomic_file;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

const ENV_ASSET_DIR: &str = "CNMPLAYER_ASSET_DIR";

pub(crate) const DEFAULT_CONFIG_TOML: &str = include_str!("../../config/default.toml");

const THEME_SYSTEM_TOML: &str = include_str!("../../themes/system.toml");
const THEME_LATTE_TOML: &str = include_str!("../../themes/catppuccin_latte.toml");
const THEME_FRAPPE_TOML: &str = include_str!("../../themes/catppuccin_frappe.toml");
const THEME_MACCHIATO_TOML: &str = include_str!("../../themes/catppuccin_macchiato.toml");
const THEME_MOCHA_TOML: &str = include_str!("../../themes/catppuccin_mocha.toml");
const THEME_AYU_LIGHT_TOML: &str = include_str!("../../themes/ayu_light.toml");
const THEME_AYU_MIRAGE_TOML: &str = include_str!("../../themes/ayu_mirage.toml");
const THEME_OCEAN_TOML: &str = include_str!("../../themes/base16_ocean.toml");
const THEME_EVERFOREST_DARK_TOML: &str = include_str!("../../themes/everforest_dark.toml");
const THEME_EVERFOREST_LIGHT_TOML: &str = include_str!("../../themes/everforest_light.toml");
const THEME_MONOKAI_PRO_TOML: &str = include_str!("../../themes/monokai_pro.toml");
const THEME_NORD_TOML: &str = include_str!("../../themes/nord.toml");
const THEME_ROSE_PINE_MOON_TOML: &str = include_str!("../../themes/rose_pine_moon.toml");
const THEME_SOLARIZED_DARK_TOML: &str = include_str!("../../themes/solarized_dark.toml");
const THEME_SOLARIZED_LIGHT_TOML: &str = include_str!("../../themes/solarized_light.toml");
const THEME_TOMORROW_LIGHT_TOML: &str = include_str!("../../themes/tomorrow_light.toml");
const THEME_TOMORROW_NIGHT_TOML: &str = include_str!("../../themes/tomorrow_night.toml");
const THEME_ZENBURN_TOML: &str = include_str!("../../themes/zenburn.toml");
const THEME_ZINC_DARK_TOML: &str = include_str!("../../themes/shadcn_zinc_dark.toml");
const THEME_ZINC_LIGHT_TOML: &str = include_str!("../../themes/shadcn_zinc_light.toml");

static ASSET_ROOT: LazyLock<PathBuf> = LazyLock::new(|| {
    std::env::var_os(ENV_ASSET_DIR)
        .map(PathBuf::from)
        .unwrap_or_else(|| STORAGE.config.clone())
});
static ASSETS_READY: LazyLock<Result<(), String>> =
    LazyLock::new(|| ensure_all_assets(&ASSET_ROOT).map_err(|error| error.to_string()));

pub fn resolve_asset_root() -> &'static Path {
    &ASSET_ROOT
}

pub fn resolve_asset_path(rel: &Path) -> PathBuf {
    resolve_asset_root().join(rel)
}

pub fn resolve_config_path() -> PathBuf {
    resolve_asset_path(Path::new("config/default.toml"))
}

pub fn ensure_assets_ready() -> Result<&'static PathBuf> {
    ASSETS_READY
        .as_ref()
        .map(|_| &*ASSET_ROOT)
        .map_err(|error| anyhow::anyhow!("{error}"))
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
    write_if_missing(&root.join("themes/ayu_light.toml"), THEME_AYU_LIGHT_TOML)?;
    write_if_missing(&root.join("themes/ayu_mirage.toml"), THEME_AYU_MIRAGE_TOML)?;
    write_if_missing(&root.join("themes/base16_ocean.toml"), THEME_OCEAN_TOML)?;
    write_if_missing(
        &root.join("themes/everforest_dark.toml"),
        THEME_EVERFOREST_DARK_TOML,
    )?;
    write_if_missing(
        &root.join("themes/everforest_light.toml"),
        THEME_EVERFOREST_LIGHT_TOML,
    )?;
    write_if_missing(
        &root.join("themes/monokai_pro.toml"),
        THEME_MONOKAI_PRO_TOML,
    )?;
    write_if_missing(&root.join("themes/nord.toml"), THEME_NORD_TOML)?;
    write_if_missing(
        &root.join("themes/rose_pine_moon.toml"),
        THEME_ROSE_PINE_MOON_TOML,
    )?;
    write_if_missing(
        &root.join("themes/solarized_dark.toml"),
        THEME_SOLARIZED_DARK_TOML,
    )?;
    write_if_missing(
        &root.join("themes/solarized_light.toml"),
        THEME_SOLARIZED_LIGHT_TOML,
    )?;
    write_if_missing(
        &root.join("themes/tomorrow_light.toml"),
        THEME_TOMORROW_LIGHT_TOML,
    )?;
    write_if_missing(
        &root.join("themes/tomorrow_night.toml"),
        THEME_TOMORROW_NIGHT_TOML,
    )?;
    write_if_missing(&root.join("themes/zenburn.toml"), THEME_ZENBURN_TOML)?;
    write_if_missing(
        &root.join("themes/shadcn_zinc_dark.toml"),
        THEME_ZINC_DARK_TOML,
    )?;
    write_if_missing(
        &root.join("themes/shadcn_zinc_light.toml"),
        THEME_ZINC_LIGHT_TOML,
    )?;

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
    atomic_file::write_atomic(path, contents.as_bytes())
}
