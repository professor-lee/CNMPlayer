use super::{GraphicsProtocol, VisualizeMode};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct GraphicsProtocolWrapper {
    protocol: GraphicsProtocol,
}

#[test]
fn graphics_protocol_keeps_legacy_values_loadable() {
    let cases = [
        ("off", GraphicsProtocol::Off),
        ("halfblocks", GraphicsProtocol::Halfblocks),
        ("auto", GraphicsProtocol::Halfblocks),
        ("sixel", GraphicsProtocol::Halfblocks),
        ("kitty", GraphicsProtocol::Halfblocks),
        ("iterm2", GraphicsProtocol::Halfblocks),
    ];

    for (raw, expected) in cases {
        let parsed: GraphicsProtocolWrapper =
            toml::from_str(&format!("protocol = \"{}\"", raw)).unwrap();
        assert_eq!(parsed.protocol, expected);
    }
}

/// 「仅歌词」曾经写作 `off`；手改或沿用旧配置的用户不能因为改名而丢档位。
#[test]
fn visualize_keeps_legacy_off_value_loadable() {
    #[derive(Debug, Deserialize)]
    struct VisualizeWrapper {
        visualize: VisualizeMode,
    }

    let cases = [
        ("lyrics", VisualizeMode::Lyrics),
        ("off", VisualizeMode::Lyrics),
        ("hidden", VisualizeMode::Hidden),
        ("bars", VisualizeMode::Bars),
        ("oscilloscope", VisualizeMode::Oscilloscope),
        ("vector", VisualizeMode::Vector),
    ];

    for (raw, expected) in cases {
        let parsed: VisualizeWrapper =
            toml::from_str(&format!("visualize = \"{}\"", raw)).unwrap();
        assert_eq!(parsed.visualize, expected);
    }
}

#[test]
fn visualize_cycles_all_modes_without_external_dependencies() {
    let modes = [
        VisualizeMode::Hidden,
        VisualizeMode::Lyrics,
        VisualizeMode::Bars,
        VisualizeMode::Oscilloscope,
        VisualizeMode::Vector,
    ];
    for (index, mode) in modes.iter().copied().enumerate() {
        assert_eq!(mode.cycle(1), modes[(index + 1) % modes.len()]);
        assert_eq!(
            mode.cycle(-1),
            modes[(index + modes.len() - 1) % modes.len()]
        );
        assert_eq!(mode.cycle(0), mode);
    }
}

#[test]
fn fps_loading_rejects_zero_and_preserves_every_positive_u32() {
    let dir =
        std::env::temp_dir().join(format!("cnmplayer-fps-config-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("default.toml");
    for fps in [0, 2, 144, u32::MAX] {
        let cfg = super::Config {
            ui_fps: fps,
            ..super::Config::default()
        };
        let raw = toml::to_string_pretty(&cfg).unwrap();
        std::fs::write(&path, &raw).unwrap();
        let loaded = super::Config::load_from_path(&path);
        if fps == 0 {
            assert!(loaded.unwrap_err().to_string().contains("ui_fps"));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        } else {
            assert_eq!(loaded.unwrap().ui_fps, fps);
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn partial_user_configuration_preserves_overrides_and_completes_nested_fields() {
    let dir = std::env::temp_dir().join(format!(
        "cnmplayer-partial-config-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("default.toml");
    std::fs::write(
        &path,
        "ui_fps = 144\ntransparent_background = false\n[cache]\nmax_age_days = 11\n",
    )
    .unwrap();
    let loaded = super::Config::load_from_path(&path).unwrap();
    assert_eq!(loaded.ui_fps, 144);
    assert!(!loaded.transparent_background);
    assert_eq!(loaded.cache.max_age_days, 11);
    let saved = std::fs::read_to_string(&path).unwrap();
    let reparsed: super::Config = toml::from_str(&saved).unwrap();
    assert_eq!(reparsed.ui_fps, 144);
    assert_eq!(reparsed.cache.max_age_days, 11);
    assert_eq!(
        toml::to_string(&loaded).unwrap(),
        toml::to_string(&reparsed).unwrap()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn invalid_user_values_are_rejected_without_defaulting_or_rewriting() {
    let dir = std::env::temp_dir().join(format!(
        "cnmplayer-invalid-values-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("default.toml");
    for raw in [
        "ui_fps = 'fast'\n",
        "cache = false\n",
        "[cache]\nmax_age_days = -1\n",
    ] {
        std::fs::write(&path, raw).unwrap();
        assert!(super::Config::load_from_path(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn corrupt_config_is_rejected_without_replacement() {
    let dir =
        std::env::temp_dir().join(format!("cnmplayer-config-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("default.toml");
    std::fs::write(&path, "not = [valid").unwrap();
    assert!(super::Config::load_from_path(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "not = [valid");
    let _ = std::fs::remove_dir_all(dir);
}
