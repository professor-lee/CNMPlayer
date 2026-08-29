use anyhow::{Context, Result};
use directories::BaseDirs;
use std::fs;
use std::path::{Path, PathBuf};

const ENV_ASSET_DIR: &str = "TMPLAYER_ASSET_DIR";

const DEFAULT_CONFIG_TOML: &str = include_str!("../../../config/default.toml");

const THEME_SYSTEM_TOML: &str = include_str!("../../../themes/system.toml");
const THEME_LATTE_TOML: &str = include_str!("../../../themes/catppuccin_latte.toml");
const THEME_FRAPPE_TOML: &str = include_str!("../../../themes/catppuccin_frappe.toml");
const THEME_MACCHIATO_TOML: &str = include_str!("../../../themes/catppuccin_macchiato.toml");
const THEME_MOCHA_TOML: &str = include_str!("../../../themes/catppuccin_mocha.toml");
const THEME_ATOM_ONE_DARK_TOML: &str = include_str!("../../../themes/atom_one_dark.toml");
const THEME_ATOM_ONE_LIGHT_TOML: &str = include_str!("../../../themes/atom_one_light.toml");

pub fn resolve_asset_root() -> PathBuf {
    if let Some(p) = std::env::var_os(ENV_ASSET_DIR) {
        return PathBuf::from(p);
    }

    if let Some(sys) = system_config_root() {
        // Always use the OS-level config directory: <config_dir>/tmplayer
        // Best-effort migration from legacy local .config, to avoid losing prior settings.
        let _ = migrate_legacy_local_assets(&sys);
        let _ = ensure_all_assets(&sys);
        return sys;
    }

    // Fallback only when the OS config directory cannot be determined.
    let local = local_config_root();
    let _ = ensure_all_assets(&local);
    local
}

pub fn resolve_asset_path(rel: &Path) -> PathBuf {
    resolve_asset_root().join(rel)
}

pub fn resolve_config_path() -> PathBuf {
    resolve_asset_path(Path::new("config/default.toml"))
}

pub fn ensure_assets_ready() -> Result<PathBuf> {
    if let Some(sys) = system_config_root() {
        // Keep behavior consistent with resolve_asset_root(): always ensure assets live here.
        let _ = migrate_legacy_local_assets(&sys);
        ensure_all_assets(&sys)?;
        return Ok(sys);
    }

    let local = local_config_root();
    ensure_all_assets(&local)?;
    Ok(local)
}

fn system_config_root() -> Option<PathBuf> {
    // Cross-platform OS config directory.
    // Linux: $XDG_CONFIG_HOME/tmplayer (usually ~/.config/tmplayer)
    // macOS: ~/Library/Application Support/tmplayer
    // Windows: %APPDATA%\tmplayer
    BaseDirs::new().map(|d| d.config_dir().join("tmplayer"))
}

fn local_config_root() -> PathBuf {
    // Legacy fallback: in the current working directory.
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".config")
}

fn migrate_legacy_local_assets(sys_root: &Path) -> Result<()> {
    // If a legacy local config exists (./.config/...), copy it into the system config
    // directory only when the system config is missing.
    let legacy_root = local_config_root();
    let legacy_cfg = legacy_root.join("config/default.toml");
    let sys_cfg = sys_root.join("config/default.toml");

    if sys_cfg.is_file() {
        return Ok(());
    }
    if !legacy_cfg.is_file() {
        return Ok(());
    }

    // Ensure destination directories exist.
    ensure_dir(&sys_root.join("config"))?;
    ensure_dir(&sys_root.join("themes"))?;

    // Copy config.
    fs::copy(&legacy_cfg, &sys_cfg)
        .with_context(|| format!("copy {} -> {}", legacy_cfg.display(), sys_cfg.display()))?;

    // Copy themes best-effort.
    let legacy_themes = legacy_root.join("themes");
    if legacy_themes.is_dir() {
        for entry in fs::read_dir(&legacy_themes)
            .with_context(|| format!("read_dir {}", legacy_themes.display()))?
        {
            let entry = entry?;
            let p = entry.path();
            if p.is_file() {
                if let Some(name) = p.file_name() {
                    let dst = sys_root.join("themes").join(name);
                    let _ = fs::copy(&p, &dst);
                }
            }
        }
    }

    Ok(())
}

fn ensure_all_assets(root: &Path) -> Result<()> {
    // Create:
    //   <root>/config/default.toml
    //   <root>/themes/*.toml
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
    write_if_missing(
        &root.join("themes/atom_one_dark.toml"),
        THEME_ATOM_ONE_DARK_TOML,
    )?;
    write_if_missing(
        &root.join("themes/atom_one_light.toml"),
        THEME_ATOM_ONE_LIGHT_TOML,
    )?;

    Ok(())
}

fn ensure_dir(p: &Path) -> Result<()> {
    fs::create_dir_all(p).with_context(|| format!("mkdir {}", p.display()))
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
