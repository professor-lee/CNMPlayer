impl DownloadRow {
    fn key(&self) -> &str {
        &self.key
    }
}

use super::super::api::ApiState;
use super::super::download::DownloadManager;
use super::super::download::DownloadPathChoice;
use super::super::download::DownloadPathError;
use super::super::download::DownloadRequest;
use super::super::download::DownloadRow;
use super::super::download::DownloadRowCache;
use super::super::download::DownloadState;
use super::super::download::DownloadTarget;
use super::super::download::STEM_BYTE_BUDGET;
use super::super::download::civil_date_from_unix_ms;
use super::super::download::download_file_stem;
use super::super::download::is_null_download_path;
use super::super::download::parse_download_path;
use super::super::download::resolve_download_root;
use super::super::download::state_glyph;
use crate::data::config::AudioQuality;
use crate::data::icons::UiIcons;
use std::path::PathBuf;
use std::time::Duration;

#[compio::test]
async fn row_status_is_loaded_by_poll_and_invalidated_after_root_change() {
    let directory =
        std::env::temp_dir().join(format!("cnmplayer-download-status-{}", std::process::id()));
    compio::fs::create_dir_all(&directory).await.unwrap();
    let target = DownloadTarget {
        dir: directory.clone(),
        base: "song".to_string(),
    };
    compio::fs::write(target.file_path("mp3"), b"audio".to_vec())
        .await
        .0
        .unwrap();
    let api = ApiState::new(None, cyper::Client::builder().build().unwrap()).unwrap();
    let mut manager = DownloadManager::new(api.clone());
    let row = DownloadRow::new("42".to_string(), target.clone());
    assert_eq!(
        manager.state_of_row(&row),
        DownloadState::NotDownloaded,
        "render-time lookup must not stat an uncached row"
    );
    let request = DownloadRequest {
        song_id: "42".to_string(),
        level: AudioQuality::Exhigh,
        title: "song".to_string(),
        artist: String::new(),
        album: String::new(),
        target: target.clone(),
    };
    let initial_version = manager.version();
    // Deliberately do not poll: the lookup stays unresolved deterministically.
    assert_eq!(manager.known_state_of("42", &target), None);
    assert!(manager.enqueue(&api, request.clone()).is_err());
    assert!(
        !manager.is_busy("42"),
        "pending lookup must not admit a download job"
    );
    assert_eq!(manager.version(), initial_version);
    assert_eq!(
        compio::fs::read(target.file_path("mp3")).await.unwrap(),
        b"audio"
    );
    compio::time::timeout(Duration::from_secs(5), async {
        while manager.state_of_row(&row) != DownloadState::Done {
            manager.poll();
            compio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        manager.known_state_of("42", &target),
        Some(DownloadState::Done)
    );
    assert!(manager.enqueue(&api, request).is_err());
    assert!(!manager.is_busy("42"));
    assert_eq!(
        compio::fs::read(target.file_path("mp3")).await.unwrap(),
        b"audio"
    );
    compio::fs::remove_file(target.file_path("mp3"))
        .await
        .unwrap();
    manager.clear_disk_cache();
    let version = manager.version();
    assert_eq!(manager.state_of_row(&row), DownloadState::NotDownloaded);
    compio::time::timeout(Duration::from_secs(5), async {
        while manager.version() == version {
            manager.poll();
            compio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(manager.state_of_row(&row), DownloadState::NotDownloaded);
    compio::runtime::spawn_blocking(move || std::fs::remove_dir_all(directory))
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn stem_skips_missing_parts() {
    assert_eq!(
        download_file_stem("歌名", "作者", "专辑"),
        "歌名 - 作者 - 专辑"
    );
    assert_eq!(download_file_stem("歌名", "作者", ""), "歌名 - 作者");
    assert_eq!(download_file_stem("歌名", "", ""), "歌名");
    assert_eq!(download_file_stem("  ", "", ""), "未知歌曲");
}

#[test]
fn stem_strips_path_separators_and_controls() {
    // 路径分隔符换 `-`（比直接吞掉可读），非法字符丢掉，控制字符丢掉。
    let stem = download_file_stem("a/b:c*d", "AC/DC", "g\nh");
    assert_eq!(stem, "a-b-cd - AC-DC - gh");
}

#[test]
fn stem_collapses_repeated_spaces() {
    assert_eq!(
        download_file_stem("Title", "Woodkid   /   Connelly", "Album"),
        "Title - Woodkid - Connelly - Album"
    );
}

/// 回归：NAME_MAX 是 255 **字节**，CJK 每字 3 字节——按字符数截断
/// 挡不住上限（长标题曾必然 ENAMETOOLONG、整首下载固定失败）。
#[test]
fn stem_respects_the_byte_budget_of_name_max() {
    let long_cjk: String = "歌".repeat(150);
    for (title, artist, album) in [
        (long_cjk.as_str(), "", ""),
        (long_cjk.as_str(), long_cjk.as_str(), long_cjk.as_str()),
        ("T", long_cjk.as_str(), long_cjk.as_str()),
    ] {
        let stem = download_file_stem(title, artist, album);
        assert!(
            stem.len() <= STEM_BYTE_BUDGET,
            "主干 {len} 字节应 ≤ {STEM_BYTE_BUDGET}",
            len = stem.len()
        );
        let path = format!("{stem}.flac.part");
        assert!(path.len() < 255, "含 .flac.part 后仍须 <255 字节");
    }

    // 截断必须落在字符边界上（结果仍是合法 UTF-8，且是原串前缀）。
    let stem = download_file_stem(&long_cjk, &long_cjk, &long_cjk);
    let joined = format!("{long_cjk} - {long_cjk} - {long_cjk}");
    assert!(joined.starts_with(&stem));

    // 常规名字不受影响。
    assert_eq!(
        download_file_stem("歌名", "作者", "专辑"),
        "歌名 - 作者 - 专辑"
    );
}

#[test]
fn civil_date_matches_known_timestamps() {
    assert_eq!(civil_date_from_unix_ms(0), None);
    assert_eq!(
        civil_date_from_unix_ms(1_600_000_000_000).as_deref(),
        Some("2020-09-13")
    );
    assert_eq!(
        civil_date_from_unix_ms(1_700_000_000_000).as_deref(),
        Some("2023-11-14")
    );
}

#[test]
fn target_paths_use_extension_and_part_suffix() {
    let target = DownloadTarget {
        dir: PathBuf::from("/tmp/cnm"),
        base: "A - B".to_string(),
    };
    assert_eq!(
        target.file_path("flac"),
        PathBuf::from("/tmp/cnm/A - B.flac")
    );
    assert_eq!(
        target.part_path("mp3"),
        PathBuf::from("/tmp/cnm/A - B.mp3.part")
    );
}

#[test]
fn state_glyph_uses_the_fixed_nerd_font_set() {
    let icons = UiIcons::new();
    let phase = Duration::ZERO;
    assert_eq!(
        state_glyph(DownloadState::NotDownloaded, phase, icons),
        '\u{ec74}'
    );
    assert_eq!(state_glyph(DownloadState::Done, phase, icons), '\u{f00c}');
    assert_eq!(
        state_glyph(DownloadState::Downloading, phase, icons),
        '\u{280b}'
    );
}

/// 预计算行的 key 必须与 `state_of` 内部构造的一致（否则磁盘缓存查不到）。
#[test]
fn row_key_matches_the_manager_key() {
    let target = DownloadTarget {
        dir: PathBuf::from("/tmp/cnm"),
        base: "A - B".to_string(),
    };
    let row = DownloadRow::new("42".to_string(), target.clone());
    assert_eq!(
        row.key(),
        target.dir.join(&target.base).display().to_string()
    );
}

/// memo 只在"列表代变化"时重建行数据、只在"任务版本变化"时重算状态。
#[test]
fn row_cache_rebuilds_only_on_epoch_and_version_changes() {
    use std::cell::Cell;

    fn refresh(
        cache: &mut DownloadRowCache,
        epoch: (u64, u64),
        version: u64,
        builds: &Cell<u32>,
        resolves: &Cell<u32>,
    ) {
        cache.refresh_inner(
            epoch,
            version,
            || {
                builds.set(builds.get() + 1);
                vec![Some(DownloadRow::new(
                    "1".to_string(),
                    DownloadTarget {
                        dir: PathBuf::from("/tmp/cnm"),
                        base: "A".to_string(),
                    },
                ))]
            },
            |_row| {
                resolves.set(resolves.get() + 1);
                DownloadState::NotDownloaded
            },
        );
    }

    let mut cache = DownloadRowCache::default();
    let builds = Cell::new(0);
    let resolves = Cell::new(0);

    refresh(&mut cache, (1, 0), 7, &builds, &resolves);
    assert_eq!((builds.get(), resolves.get()), (1, 1));
    assert_eq!(cache.state_at(0), Some(DownloadState::NotDownloaded));

    // 同代同版本：什么都不做。
    refresh(&mut cache, (1, 0), 7, &builds, &resolves);
    assert_eq!((builds.get(), resolves.get()), (1, 1), "不该重建/重算");

    // 任务事件（版本 +1）：只重算状态。
    refresh(&mut cache, (1, 0), 8, &builds, &resolves);
    assert_eq!((builds.get(), resolves.get()), (1, 2), "只重算状态");

    // 换列表（代 +1）：重建行数据并重算状态。
    refresh(&mut cache, (2, 0), 8, &builds, &resolves);
    assert_eq!((builds.get(), resolves.get()), (2, 3), "换列表要重建行数据");

    // 回归：两个计数器同时 +1。旧实现把两者异或折叠成一个 u64，
    // 2^0 == 3^1 == 2，键不变 → 行数据陈旧（图标错位）；二元组必须照常重建。
    refresh(&mut cache, (3, 1), 8, &builds, &resolves);
    assert_eq!(
        (builds.get(), resolves.get()),
        (3, 4),
        "列表代与根目录代同增时必须重建行数据"
    );

    assert_eq!(cache.state_at(9), None, "越界行没有图标");
}

#[test]
fn parse_rejects_relative_paths() {
    assert_eq!(
        parse_download_path("relative/dir"),
        Err(DownloadPathError::NotAbsolute)
    );
    assert_eq!(
        parse_download_path("  相对/路径  "),
        Err(DownloadPathError::NotAbsolute)
    );
}

/// 留空 = 按需求回填成 `Null`（禁用下载），不是"保留旧值"。
#[test]
fn empty_path_falls_back_to_null() {
    assert_eq!(parse_download_path(""), Ok(DownloadPathChoice::Disabled));
    assert_eq!(
        parse_download_path("   \t "),
        Ok(DownloadPathChoice::Disabled)
    );
}

/// `Null` 是"显式禁用下载"的哨兵：解析成 Disabled，解析根目录得到 None。
#[test]
fn null_path_disables_downloads() {
    assert_eq!(
        parse_download_path("Null"),
        Ok(DownloadPathChoice::Disabled)
    );
    assert_eq!(
        parse_download_path("  null  "),
        Ok(DownloadPathChoice::Disabled)
    );
    assert!(is_null_download_path("NULL"));
    assert!(!is_null_download_path("Nullx"));
    assert_eq!(resolve_download_root(Some("Null")), None);
    assert_eq!(resolve_download_root(Some("null")), None);
}
