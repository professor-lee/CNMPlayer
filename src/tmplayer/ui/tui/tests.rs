use super::input::{
    contains, hit_test, progress_for_drag, volume_at, volume_for_drag, wheel_over_playlist,
};
use super::layout::ModalRows;
use super::modals::download::render_download_settings_modal;
use super::modals::help::{help_item_count, help_items, render_help_modal};
use super::modals::lyrics::render_lyrics_settings_modal;
use super::modals::settings::{render_general_settings_modal, render_settings_modal};
use super::page::{cover_key, slide_page_from_bottom};
use super::*;
use crate::data::icons::UiIcons;
use crate::render::cover_pipeline::CoverKey;
use crate::render::drawer::translated_clip;
use crate::tmplayer::app::state::Overlay;
use crate::tmplayer::ui::components::control_buttons;
use crate::tmplayer::ui::panels::info_panel::{download_cells, heart_cells};
use crate::tmplayer::ui::panels::{info_panel, playlist_panel};
use crate::tmplayer::utils::input::Action;
use crate::ui::theme::{ColorCapability, Theme, ThemePalette};
use ratatui::style::Style;

#[test]
fn fullscreen_drawer_translates_content_and_keeps_cover_geometry() {
    use crate::data::config::VisualizeMode;
    use crate::render::cover_pipeline::CoverStatus;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;

    let mut app = state(Overlay::None);
    app.config.show_hints = false;
    app.config.visualize = VisualizeMode::Lyrics;
    // TestBackend does not emulate a terminal erasing a wide glyph's tail.
    // Compare full terminal diffs with narrow text; wide cells are tested below.
    app.language = crate::data::config::Language::En;
    app.player.track.title = "Drawer song".into();
    app.player.track.artist = "Drawer artist".into();
    app.player.track.cover = Some(encoded_cover().into());
    app.player.track.cover_hash = Some(700);
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let mut covers = CoverPipeline::new(WakeSignal::default());
    let mut drawer = PlaylistDrawer::default();

    // The first, fully offscreen frame still starts real cover preparation.
    let layout = draw_page_reveal(&mut terminal, &mut app, &mut covers, &mut drawer, 0.0).unwrap();
    let content = info_panel::cover_content_rect(info_panel::layout(layout.left, 120).cover);
    let key = cover_key(content, 700);
    assert_eq!(covers.status(key), CoverStatus::Loading);
    assert!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .all(|cell| *cell == Cell::default())
    );

    wait_ready(&mut covers, key);
    draw_page(&mut terminal, &mut app, &mut covers, &mut drawer).unwrap();
    let full = terminal.backend().buffer().clone();
    let steady_cover = covers.frame(key).unwrap().clone();

    // Open from below, then close downwards. Row offsets are worked examples
    // at height 40, independent of the implementation's movement calculation.
    for (progress, offset) in [
        (0.025, 39),
        (0.2, 32),
        (0.5, 20),
        (0.8, 8),
        (1.0, 0),
        (0.8, 8),
        (0.5, 20),
        (0.2, 32),
        (0.0, 40),
    ] {
        draw_page_reveal(&mut terminal, &mut app, &mut covers, &mut drawer, progress).unwrap();
        let actual = terminal.backend().buffer();
        for y in 0..40 {
            for x in 0..120 {
                if y < offset {
                    assert_eq!(
                        actual[(x, y)],
                        Cell::default(),
                        "vacated row, progress={progress}"
                    );
                } else {
                    assert_eq!(
                        actual[(x, y)],
                        full[(x, y - offset)],
                        "whole page translates: progress={progress}, cell=({x},{y})"
                    );
                }
            }
        }
        assert_eq!(covers.status(key), CoverStatus::Ready);
        assert_eq!(
            covers.frame(key),
            Some(&steady_cover),
            "movement must reuse final-size chafa cells"
        );
    }
}

#[test]
fn drawer_moves_wide_glyph_and_colors_with_a_nonzero_origin() {
    let mut buffer = Buffer::empty(Rect::new(7, 9, 6, 4));
    let style = Style::default()
        .fg(ratatui::style::Color::Rgb(200, 50, 30))
        .bg(ratatui::style::Color::Rgb(20, 40, 60));
    buffer.set_string(7, 9, "中AB", style);
    let source = buffer.clone();
    slide_page_from_bottom(&mut buffer, 0.5, None);
    assert_eq!(buffer[(7, 11)].symbol(), "中");
    for x in 7..13 {
        assert_eq!(buffer[(x, 11)], source[(x, 9)]);
        assert_eq!(buffer[(x, 12)], source[(x, 10)]);
        assert_eq!(buffer[(x, 9)], ratatui::buffer::Cell::default());
        assert_eq!(buffer[(x, 10)], ratatui::buffer::Cell::default());
    }
}

#[test]
fn fullscreen_handoff_uses_one_terminal_and_a_fresh_exit_snapshot() {
    use crate::{data::config::VisualizeMode, render::snapshot};
    use ratatui::backend::TestBackend;

    fn paint_host(frame: &mut ratatui::Frame, label: &str) {
        for y in frame.area().top()..frame.area().bottom() {
            let area = frame.area();
            frame
                .buffer_mut()
                .set_string(area.x, y, format!("{label} row {y}"), Style::default());
        }
    }

    let mut app = state(Overlay::None);
    app.config.show_hints = false;
    app.config.small_window_display = false;
    app.config.visualize = VisualizeMode::Lyrics;
    app.language = crate::data::config::Language::En;
    app.player.track.title = "Fullscreen content".into();
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal
        .draw(|frame| paint_host(frame, "HOST BEFORE"))
        .unwrap();
    let visible_host = terminal.backend().buffer().clone();
    let host = snapshot::capture(&mut terminal, |frame| paint_host(frame, "HOST BEFORE")).unwrap();
    assert_eq!(
        terminal.backend().buffer(),
        &visible_host,
        "offscreen capture cannot submit or clear the Host"
    );

    let mut tui = Tui::new(&mut terminal, WakeSignal::default());
    tui.draw(&mut app).unwrap();
    let full = tui.terminal_mut().backend().buffer().clone();
    for (progress, offset) in [(0.0, 40), (0.2, 32), (0.5, 20), (0.8, 8), (1.0, 0)] {
        tui.draw_reveal(&mut app, &host, progress).unwrap();
        let actual = tui.terminal_mut().backend().buffer();
        for y in 0..40 {
            for x in 0..120 {
                let expected = if y < offset {
                    &host[(x, y)]
                } else {
                    &full[(x, y - offset)]
                };
                assert_eq!(
                    &actual[(x, y)],
                    expected,
                    "entry frame {progress}, ({x}, {y})"
                );
            }
        }
    }
    drop(host);

    // The Host changed while Fullscreen was open. Capture it offscreen,
    // leaving the current Fullscreen frame intact until sliding starts.
    let exit_host =
        snapshot::capture(tui.terminal_mut(), |frame| paint_host(frame, "HOST AFTER")).unwrap();
    assert_eq!(tui.terminal_mut().backend().buffer(), &full);
    for (progress, offset) in [(1.0, 0), (0.8, 8), (0.5, 20), (0.2, 32), (0.0, 40)] {
        tui.draw_reveal(&mut app, &exit_host, progress).unwrap();
        let actual = tui.terminal_mut().backend().buffer();
        for y in 0..40 {
            for x in 0..120 {
                let expected = if y < offset {
                    &exit_host[(x, y)]
                } else {
                    &full[(x, y - offset)]
                };
                assert_eq!(
                    &actual[(x, y)],
                    expected,
                    "exit frame {progress}, ({x}, {y})"
                );
            }
        }
    }
    drop(exit_host);
    drop(tui);
    terminal
        .draw(|frame| paint_host(frame, "HOST RESUMED"))
        .unwrap();
    for (x, ch) in "HOST RESUMED row 0".chars().enumerate() {
        assert_eq!(
            terminal.backend().buffer()[(x as u16, 0)].symbol(),
            ch.to_string()
        );
    }
}

fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn state(overlay: Overlay) -> AppState {
    let theme = Theme {
        name: "system".to_string(),
        palette: ThemePalette {
            text: (255, 255, 255),
            subtext: (128, 128, 128),
            base: (0, 0, 0),
            surface: (16, 16, 16),
            buff: (64, 64, 64),
            accent: (255, 0, 0),
            accent2: (0, 255, 0),
            accent3: (0, 0, 255),
        },
        capability: ColorCapability::TrueColor,
    };
    let mut app = AppState::new(
        crate::data::config::Config::default(),
        theme,
        crate::data::config::Language::Zh,
    );
    app.overlay = overlay;
    app
}

/// 控件矩形都在左栏同一列上，互不重叠；用于验证"点击到底落在哪"。
fn page_layout() -> UiLayout {
    UiLayout {
        info_controls: rect(0, 0, 30, 1),
        info_progress: rect(0, 1, 30, 1),
        info_volume: rect(0, 2, 30, 1),
        playlist_list_inner: rect(0, 3, 30, 4),
        ..UiLayout::default()
    }
}

/// 弹窗盖住整页：未命中弹窗自身的点击必须被吞掉，不许穿透到底层控件。
#[test]
fn modal_overlays_swallow_page_clicks() {
    let layout = page_layout();
    // 播放列表命中要按渲染窗口换算：给一行都放得下的窗口。
    let mut plain = state(Overlay::None);
    plain.playlist_list_scroll = 0;
    plain.playlist_list_rows = layout.playlist_list_inner.height as usize;

    // 先确认坐标确实压在活控件上，否则下面的断言是空的
    assert!(matches!(
        hit_test(&layout, &plain, 0, 1),
        Some(Action::SeekToFraction(_))
    ));
    assert!(matches!(
        hit_test(&layout, &plain, 0, 2),
        Some(Action::SetVolume(_))
    ));
    assert!(matches!(
        hit_test(&layout, &plain, 0, 3),
        Some(Action::PlaylistSelect(_))
    ));

    for overlay in [
        Overlay::SettingsModal,
        Overlay::GeneralSettingsModal,
        Overlay::MprisNameEditModal,
        Overlay::BarSettingsModal,
        Overlay::AboutModal,
        Overlay::HelpModal,
        Overlay::EqModal,
    ] {
        let app = state(overlay);
        for row in [0u16, 1, 2, 3] {
            assert_eq!(
                hit_test(&layout, &app, 0, row),
                None,
                "{overlay:?} 在 ({0},{row}) 的点击应被吞掉",
                0
            );
        }
    }
}

/// 播放列表面板只占左栏，不拦整页。
#[test]
fn playlist_overlay_keeps_the_page_clickable() {
    let layout = page_layout();
    let app = state(Overlay::Playlist);

    assert_eq!(
        hit_test(&layout, &app, 0, 1),
        Some(Action::SeekToFraction(0.0))
    );
}

/// 编辑态只有路径值段落换底色（标签保持 modal 底色）。
#[test]
fn download_path_edit_paints_only_the_value_area() {
    let mut app = state(Overlay::DownloadSettingsModal);
    app.download_root = Some(std::path::PathBuf::from("/tmp/cnmplayer"));
    app.download_path_edit = Some(crate::app::settings_controller::SettingsTextEdit {
        buffer: "/tmp/cnmplayer".to_string(),
        cursor: 5,
        window_col: 0,
    });

    let (rows, buf) = render_to_buffer_sized(80, 24, &mut app, |f, app, rows| {
        render_download_settings_modal(f, f.area(), app, rows)
    });

    let path_row = rows.get(1).expect("路径行");
    let label_x = path_row.x + 2;
    let value_x = label_x + 10; // "下载路径" 4 个 CJK（8 列）+ ": "
    let surface = app.theme.color_surface();
    let buff = app.theme.color_buff();

    let label_cell = &buf[(label_x, path_row.y)];
    assert_eq!(label_cell.symbol(), "下");
    assert_eq!(label_cell.style().bg, Some(surface), "标签不换底色");

    let value_cell = &buf[(value_x, path_row.y)];
    assert_eq!(value_cell.symbol(), "/");
    assert_eq!(value_cell.style().bg, Some(buff), "路径值段落换底色");
    assert_eq!(value_cell.style().fg, Some(app.theme.color_text()));
}
#[test]
fn rendered_title_icons_match_mouse_hits_in_nerd_and_ascii_modes() {
    use crate::tmplayer::DownloadIconState;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    for liked in [false, true] {
        for download in [
            DownloadIconState::NotDownloaded,
            DownloadIconState::Downloading,
            DownloadIconState::Done,
            DownloadIconState::Hidden,
        ] {
            let mut app = state(Overlay::None);
            app.player.liked = liked;
            app.player.track.title = "Title".to_string();
            app.download_state = download;
            let (_, buf) = render_to_buffer_sized(120, 40, &mut app, |f, app, _| {
                info_panel::render(f, f.area(), 120, app);
            });
            let meta = info_panel::layout(rect(0, 0, 120, 40), 120).meta;
            let layout = UiLayout {
                info_meta: meta,
                ..UiLayout::default()
            };
            let click = |col, row| {
                let event = MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: col,
                    row,
                    modifiers: KeyModifiers::empty(),
                };
                assert_eq!(
                    crate::tmplayer::utils::input::map_mouse(event),
                    Action::MouseClick { col, row }
                );
                hit_test(&layout, &app, col, row)
            };
            let (heart_x, y, heart_width) = heart_cells(meta, &app).unwrap();
            let heart: String = (heart_x..heart_x + heart_width)
                .map(|x| buf[(x, y)].symbol())
                .collect();
            assert_eq!(heart, crate::data::icons::UiIcons::new().heart(liked));
            for x in heart_x..heart_x + heart_width {
                assert_eq!(click(x, y), Some(Action::ToggleFavorite));
                assert_eq!(click(x, y + 1), None);
            }
            if let Some((download_x, download_y, width)) = download_cells(meta, &app) {
                assert_eq!(download_y, y);
                assert_eq!(download_x + width + 1, heart_x);
                assert_eq!(
                    buf[(download_x, y)].fg,
                    app.theme.color_subtext(),
                    "下载三态均使用歌曲列表未下载图标的颜色"
                );
                for x in download_x..download_x + width {
                    assert_eq!(click(x, y), Some(Action::ToggleDownload));
                }
            } else {
                assert_eq!(download, DownloadIconState::Hidden);
                assert_eq!(click(heart_x - 2, y), None);
            }
            assert_eq!(buf[(heart_x - 1, y)].symbol(), " ");
            assert_eq!(click(heart_x - 1, y), None);
        }
    }
}

#[test]
fn title_icon_geometry_omits_undrawn_or_clipped_glyphs() {
    let mut app = state(Overlay::None);
    app.download_state = crate::tmplayer::DownloadIconState::NotDownloaded;
    for meta in [Rect::default(), rect(3, 7, 0, 3), rect(3, 7, 12, 0)] {
        assert_eq!(heart_cells(meta, &app), None);
        assert_eq!(download_cells(meta, &app), None);
        let layout = UiLayout {
            info_meta: meta,
            ..UiLayout::default()
        };
        assert_eq!(hit_test(&layout, &app, meta.x, meta.y), None);
    }
    let width =
        unicode_width::UnicodeWidthStr::width(crate::data::icons::UiIcons::new().heart(false))
            as u16;
    assert_eq!(heart_cells(rect(0, 0, width, 1), &app), Some((0, 0, width)));
    assert_eq!(heart_cells(rect(0, 0, width - 1, 1), &app), None);
    assert_eq!(download_cells(rect(0, 0, width + 1, 1), &app), None);
}

fn encoded_cover() -> Vec<u8> {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(32, 32, |x, y| {
        image::Rgb([(x * 7) as u8, (y * 7) as u8, ((x + y) * 3) as u8])
    }));
    let mut encoded = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    encoded.into_inner()
}

fn wait_ready(covers: &mut CoverPipeline, key: CoverKey) {
    use crate::render::cover_pipeline::CoverStatus;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while covers.status(key) != CoverStatus::Ready {
        assert!(
            std::time::Instant::now() < deadline,
            "cover preparation timed out"
        );
        covers.poll();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn halfblocks_loading_missing_and_transition_never_emit_ascii_art() {
    use crate::data::config::GraphicsProtocol;
    use crate::tmplayer::app::state::CoverSnapshot;
    use ratatui::backend::TestBackend;
    let mut app = state(Overlay::None);
    app.config.graphics_protocol = GraphicsProtocol::Halfblocks;
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let mut covers = CoverPipeline::new(WakeSignal::default());
    let mut drawer = PlaylistDrawer::default();
    let layout = draw_page(&mut terminal, &mut app, &mut covers, &mut drawer).unwrap();
    let cover = info_panel::layout(layout.left, 120).cover;
    let assert_blank = |buffer: &Buffer| {
        for y in cover.top()..cover.bottom() {
            for x in cover.left()..cover.right() {
                assert_eq!(buffer[(x, y)].symbol(), " ");
            }
        }
    };
    assert_blank(terminal.backend().buffer());
    let from = CoverSnapshot::from(&app.player.track);
    app.player.track.cover = Some(encoded_cover().into());
    app.player.track.cover_hash = Some(42);
    app.start_cover_anim(
        from,
        CoverSnapshot::from(&app.player.track),
        -1,
        app.last_frame,
    );
    draw_page(&mut terminal, &mut app, &mut covers, &mut drawer).unwrap();
    assert_blank(terminal.backend().buffer());
    let content = info_panel::cover_content_rect(cover);
    wait_ready(&mut covers, cover_key(content, 42));
    app.cover_anim = None;
    draw_page(&mut terminal, &mut app, &mut covers, &mut drawer).unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .any(|cell| matches!(cell.fg, ratatui::style::Color::Rgb(..)))
    );
}

#[test]
fn sidebar_slides_preserve_exposed_song_chafa_cells() {
    use crate::data::config::GraphicsProtocol;
    use crate::render::cover_pipeline::CoverStatus;
    use ratatui::backend::TestBackend;
    use std::io::Cursor;
    use std::time::{Duration, Instant};

    let mut app = state(Overlay::None);
    app.config.graphics_protocol = GraphicsProtocol::Halfblocks;
    app.config.show_hints = false;
    let image = image::RgbImage::from_fn(32, 32, |x, y| {
        image::Rgb([(x * 7) as u8, (y * 7) as u8, ((x + y) * 3) as u8])
    });
    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    app.player.track.cover = Some(encoded.into_inner().into());
    app.player.track.cover_hash = Some(42);
    let mut halfblocks = CoverPipeline::new(WakeSignal::default());
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let mut sidebar = PlaylistDrawer::default();
    let layout = draw_page(&mut terminal, &mut app, &mut halfblocks, &mut sidebar).unwrap();
    let loading = terminal.backend().buffer().clone();

    // Before worker results are consumed there is no ASCII fallback.
    for y in info_panel::cover_content_rect(info_panel::layout(layout.left, 120).cover).top()
        ..info_panel::cover_content_rect(info_panel::layout(layout.left, 120).cover).bottom()
    {
        for x in info_panel::cover_content_rect(info_panel::layout(layout.left, 120).cover).left()
            ..info_panel::cover_content_rect(info_panel::layout(layout.left, 120).cover).right()
        {
            assert_eq!(loading[(x, y)].symbol(), " ");
        }
    }
    let content =
        info_panel::cover_content_rect(info_panel::layout(layout.left, layout.full.width).cover);
    let deadline = Instant::now() + Duration::from_secs(2);
    while halfblocks.status(cover_key(content, 42)) != CoverStatus::Ready {
        assert!(
            Instant::now() < deadline,
            "song cover preparation timed out"
        );
        halfblocks.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    draw_page(&mut terminal, &mut app, &mut halfblocks, &mut sidebar).unwrap();
    let closed = terminal.backend().buffer().clone();
    assert_ne!(closed, loading, "ready image replaces blank loading area");

    for opening in [true, false] {
        app.overlay = if opening {
            Overlay::Playlist
        } else {
            Overlay::None
        };
        app.playlist_slide_target_x = if opening {
            0
        } else {
            -(layout.left.width as i16)
        };
        for visible in [1, 8, 16, 24, 32, 39, 40] {
            app.playlist_slide_x = visible - layout.left.width as i16;
            let frame = draw_page(&mut terminal, &mut app, &mut halfblocks, &mut sidebar).unwrap();
            let buffer = terminal.backend().buffer();
            for y in content.top()..content.bottom() {
                for x in content.left()..content.right() {
                    if !contains(frame.playlist_rect, x, y) {
                        assert_eq!(
                            buffer[(x, y)],
                            closed[(x, y)],
                            "exposed song cell ({x}, {y}), opening={opening}, visible={visible}"
                        );
                    } else {
                        assert_ne!(
                            buffer[(x, y)],
                            closed[(x, y)],
                            "sidebar must occlude the song image"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn drawer_prewarms_cover_and_translates_fixed_content_in_both_directions() {
    use crate::data::config::GraphicsProtocol;
    use crate::tmplayer::data::playlist::PlaylistItem;
    use ratatui::backend::TestBackend;
    use std::io::Cursor;

    let mut app = state(Overlay::None);
    app.config.graphics_protocol = GraphicsProtocol::Halfblocks;
    app.config.show_hints = false;
    let image = image::RgbImage::from_fn(24, 24, |x, y| {
        image::Rgb([(x * 9) as u8, (y * 9) as u8, ((x + y) * 5) as u8])
    });
    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    app.playlist_cover = Some(encoded.into_inner().into());
    app.playlist_cover_hash = Some(91);
    app.playlist_view.items = (0..12)
        .map(|i| PlaylistItem {
            song_id: Some(i.to_string()),
            title: format!("Track {i}"),
        })
        .collect();
    let mut song_covers = CoverPipeline::new(WakeSignal::default());
    let mut drawer = PlaylistDrawer::default();

    // Resize uses a new final-size cover, not an animation-sized image.
    for (width, height) in [(120, 40), (90, 30)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        app.overlay = Overlay::None;
        app.playlist_slide_x = 0;
        app.playlist_slide_target_x = 0;
        let closed_layout =
            draw_page(&mut terminal, &mut app, &mut song_covers, &mut drawer).unwrap();
        let background = terminal.backend().buffer().clone();
        let final_layout = playlist_panel::compute_layout(closed_layout.left, &app);
        wait_ready(&mut song_covers, cover_key(final_layout.cover_rect, 91));
        app.overlay = Overlay::Playlist;
        draw_page(&mut terminal, &mut app, &mut song_covers, &mut drawer).unwrap();
        let fully_open = terminal.backend().buffer().clone();
        let cover_cell = &fully_open[(final_layout.cover_rect.x, final_layout.cover_rect.y)];
        assert_ne!(
            cover_cell.fg,
            app.theme.color_text(),
            "prewarmed chafa is not ASCII"
        );

        for opening in [true, false] {
            app.overlay = if opening {
                Overlay::Playlist
            } else {
                Overlay::None
            };
            app.playlist_slide_target_x = if opening {
                0
            } else {
                -(closed_layout.left.width as i16)
            };
            for dx in [-(closed_layout.left.width as i16), -17, -8, -1, 0] {
                app.playlist_slide_x = dx;
                let frame =
                    draw_page(&mut terminal, &mut app, &mut song_covers, &mut drawer).unwrap();
                let actual = terminal.backend().buffer();
                for y in closed_layout.left.top()..closed_layout.left.bottom() {
                    for x in closed_layout.left.left()..closed_layout.left.right() {
                        if contains(frame.playlist_rect, x, y) {
                            let source_x = (i32::from(x) - i32::from(dx)) as u16;
                            assert_eq!(
                                actual[(x, y)],
                                fully_open[(source_x, y)],
                                "drawer cell must translate, not rescale: opening={opening}, dx={dx}"
                            );
                        } else {
                            assert_eq!(
                                actual[(x, y)],
                                background[(x, y)],
                                "outside drawer is unchanged"
                            );
                        }
                    }
                }
                if !frame.playlist_list_inner.is_empty() {
                    let hit = hit_test(
                        &frame,
                        &app,
                        frame.playlist_list_inner.x,
                        frame.playlist_list_inner.y,
                    );
                    assert_eq!(hit, Some(Action::PlaylistSelect(app.playlist_list_scroll)),);
                    assert_eq!(
                        wheel_over_playlist(
                            &frame,
                            &app,
                            frame.playlist_rect.x,
                            frame.playlist_rect.y
                        ),
                        opening
                    );
                }
            }
        }
    }
}

#[test]
fn drawer_failed_cover_is_blank_without_ascii_or_retry_loop() {
    use crate::data::config::GraphicsProtocol;
    use crate::render::cover_pipeline::CoverStatus;
    use ratatui::backend::TestBackend;
    let mut app = state(Overlay::Playlist);
    app.config.graphics_protocol = GraphicsProtocol::Halfblocks;
    app.playlist_cover = Some(b"invalid image".to_vec().into());
    app.playlist_cover_hash = Some(19);
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let mut covers = CoverPipeline::new(WakeSignal::default());
    let mut drawer = PlaylistDrawer::default();
    let layout = draw_page(&mut terminal, &mut app, &mut covers, &mut drawer).unwrap();
    let cover = playlist_panel::compute_layout(layout.left, &app).cover_rect;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while covers.status(cover_key(cover, 19)) != CoverStatus::Failed {
        assert!(std::time::Instant::now() < deadline);
        covers.poll();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    for dx in [0, -8, -17] {
        app.playlist_slide_x = dx;
        let frame = draw_page(&mut terminal, &mut app, &mut covers, &mut drawer).unwrap();
        let exposed = translated_clip(cover, i32::from(dx), frame.playlist_rect);
        for y in exposed.top()..exposed.bottom() {
            for x in exposed.left()..exposed.right() {
                assert_eq!(terminal.backend().buffer()[(x, y)].symbol(), " ");
            }
        }
    }
}

#[test]
fn rendered_controls_match_nerd_and_ascii_glyph_widths() {
    use crate::tmplayer::app::state::{PlaybackState, RepeatMode};
    use unicode_width::UnicodeWidthStr;

    for playing in [false, true] {
        for repeat in [
            RepeatMode::Sequence,
            RepeatMode::Shuffle,
            RepeatMode::LoopAll,
            RepeatMode::LoopOne,
        ] {
            let mut app = state(Overlay::None);
            app.player.playback = if playing {
                PlaybackState::Playing
            } else {
                PlaybackState::Paused
            };
            app.player.repeat_mode = repeat;
            let icons = UiIcons::new();
            let labels = [
                icons.previous(),
                icons.play_pause(playing),
                icons.next(),
                match repeat {
                    RepeatMode::Sequence => icons.sequence(),
                    RepeatMode::Shuffle => icons.shuffle(),
                    RepeatMode::LoopAll => icons.loop_all(),
                    RepeatMode::LoopOne => icons.loop_one(),
                },
            ];
            let actions = [
                Action::Prev,
                Action::TogglePlayPause,
                Action::Next,
                Action::ToggleRepeatMode,
            ];
            let line_width = labels.iter().map(|label| label.width()).sum::<usize>() as u16 + 3;
            for area_width in [120, 7] {
                let controls = rect(0, 0, area_width, 1);
                let layout = UiLayout {
                    info_controls: controls,
                    ..UiLayout::default()
                };
                let (_, buffer) = render_to_buffer_sized(120, 1, &mut app, |f, app, _| {
                    control_buttons::render(f, controls, app);
                });
                let mut x = (area_width / 2).saturating_sub(line_width.min(area_width) / 2);
                for (label, action) in labels.into_iter().zip(actions) {
                    for ch in label.chars() {
                        if x < area_width {
                            assert_eq!(buffer[(x, 0)].symbol(), ch.to_string());
                            assert_eq!(hit_test(&layout, &app, x, 0), Some(action));
                        }
                        x += 1;
                    }
                    if x < area_width {
                        assert_eq!(
                            hit_test(&layout, &app, x, 0),
                            None,
                            "control separator is not clickable"
                        );
                    }
                    x += 1;
                }
                assert_eq!(
                    hit_test(&layout, &app, area_width, 0),
                    None,
                    "clipped control is not clickable"
                );
            }
        }
    }
}

/// 作者名贴 meta 块第 2 行左端：只有名字画出来的那几格可点，行尾空白不算。
#[test]
fn clicking_the_artist_row_opens_the_author_page() {
    let layout = UiLayout {
        info_meta: rect(2, 5, 20, 3),
        ..UiLayout::default()
    };
    let mut app = state(Overlay::None);
    app.player.track.artist = "Jay".to_string();

    assert_eq!(
        hit_test(&layout, &app, 2, 6),
        Some(Action::OpenAuthorPage(0))
    );
    assert_eq!(
        hit_test(&layout, &app, 4, 6),
        Some(Action::OpenAuthorPage(0))
    );
    assert_eq!(hit_test(&layout, &app, 5, 6), None, "作者行行尾空白不算");
    assert_eq!(hit_test(&layout, &app, 2, 5), None, "标题行不是作者行");
    assert_ne!(
        hit_test(&layout, &app, 2, 7),
        Some(Action::OpenAuthorPage(0)),
        "专辑行不打开作者页"
    );
}

/// 多作者（"A / B"）：按字符位置分段，点谁的名字进谁的页面，连接符不算。
#[test]
fn clicking_each_artist_segment_opens_that_artist() {
    let layout = UiLayout {
        info_meta: rect(2, 5, 20, 3),
        ..UiLayout::default()
    };
    let mut app = state(Overlay::None);
    app.player.track.artist = "Caffeine / 初音ミク".to_string();

    // 行首在 x=2："Caffeine" 占 2..=9，" / " 占 10..=12，"初音ミク" 占 13..=20。
    assert_eq!(
        hit_test(&layout, &app, 2, 6),
        Some(Action::OpenAuthorPage(0))
    );
    assert_eq!(
        hit_test(&layout, &app, 9, 6),
        Some(Action::OpenAuthorPage(0))
    );
    assert_eq!(hit_test(&layout, &app, 10, 6), None, "连接符不是作者名");
    assert_eq!(hit_test(&layout, &app, 12, 6), None, "连接符不是作者名");
    assert_eq!(
        hit_test(&layout, &app, 13, 6),
        Some(Action::OpenAuthorPage(1))
    );
    assert_eq!(
        hit_test(&layout, &app, 20, 6),
        Some(Action::OpenAuthorPage(1))
    );
    assert_eq!(hit_test(&layout, &app, 21, 6), None, "第二个名字画完了");

    // 行宽不足以放下整段时，只算画出来的那几格（"初" 两格，"音" 放不下）。
    let narrow = UiLayout {
        info_meta: rect(0, 0, 13, 3),
        ..UiLayout::default()
    };
    assert_eq!(
        hit_test(&narrow, &app, 12, 1),
        Some(Action::OpenAuthorPage(1))
    );
    assert_eq!(hit_test(&narrow, &app, 13, 1), None, "meta 只有 13 格");
}

/// 专辑名贴 meta 块第 3 行左端：同样只算画出来的字符。
#[test]
fn clicking_the_album_row_opens_the_album_page() {
    let layout = UiLayout {
        info_meta: rect(2, 5, 20, 3),
        ..UiLayout::default()
    };
    let mut app = state(Overlay::None);
    app.player.track.artist.clear();
    app.player.track.album = "Album".to_string();

    assert_eq!(hit_test(&layout, &app, 2, 7), Some(Action::OpenAlbumPage));
    assert_eq!(hit_test(&layout, &app, 6, 7), Some(Action::OpenAlbumPage));
    assert_eq!(hit_test(&layout, &app, 7, 7), None, "专辑行行尾空白不算");
    assert_eq!(
        hit_test(&layout, &app, 2, 6),
        None,
        "作者行走作者页（这里作者名为空），不是专辑页"
    );

    let two_rows = UiLayout {
        info_meta: rect(2, 5, 20, 2),
        ..UiLayout::default()
    };
    assert_eq!(
        hit_test(&two_rows, &app, 2, 7),
        None,
        "meta 只有两行时专辑行没画"
    );
}

#[test]
fn playlist_panel_takes_the_clicks_over_the_names_it_covers() {
    // 浮层整块盖住左栏（无封面时列表内区就是整条左栏）：它的行命中排在最后，
    // 但被它盖住的作者名/专辑名必须让位，否则点在可见的歌单行上会退出全屏页。
    let mut app = state(Overlay::Playlist);
    app.player.track.artist = "Jay".to_string();
    app.player.track.album = "Fantasy".to_string();
    app.playlist_list_scroll = 0;
    app.playlist_list_rows = 10;
    let layout = UiLayout {
        info_meta: rect(2, 5, 20, 3),
        playlist_rect: rect(0, 4, 24, 12),
        playlist_list_inner: rect(1, 5, 22, 10),
        ..UiLayout::default()
    };

    assert_eq!(
        hit_test(&layout, &app, 2, 6),
        Some(Action::PlaylistSelect(1))
    );
    assert_eq!(
        hit_test(&layout, &app, 2, 7),
        Some(Action::PlaylistSelect(2))
    );

    // 浮层收起（滑出动画结束）后名字重新可点：命中区只看实际绘制的那块矩形。
    let uncovered = UiLayout {
        info_meta: rect(2, 5, 20, 3),
        ..UiLayout::default()
    };
    assert_eq!(
        hit_test(&uncovered, &app, 2, 6),
        Some(Action::OpenAuthorPage(0))
    );
    assert_eq!(
        hit_test(&uncovered, &app, 2, 7),
        Some(Action::OpenAlbumPage)
    );
}

#[test]
fn playlist_panel_blocks_hidden_controls_but_not_uncovered_cells() {
    let mut app = state(Overlay::Playlist);
    app.download_state = crate::tmplayer::DownloadIconState::NotDownloaded;
    app.playlist_list_scroll = 7;
    app.playlist_list_rows = 5;
    let layout = UiLayout {
        playlist_rect: rect(0, 0, 24, 8),
        playlist_list_inner: rect(1, 1, 22, 6),
        info_meta: rect(1, 1, 22, 3),
        info_controls: rect(0, 4, 30, 1),
        info_volume: rect(0, 5, 30, 1),
        info_progress: rect(0, 6, 30, 1),
        ..UiLayout::default()
    };
    for row in 1..=5 {
        assert_eq!(
            hit_test(&layout, &app, 21, row),
            Some(Action::PlaylistSelect(7 + usize::from(row - 1)))
        );
    }
    assert_eq!(
        hit_test(&layout, &app, 21, 6),
        None,
        "footer swallows the hidden seek bar"
    );
    assert_eq!(
        hit_test(&layout, &app, 0, 4),
        None,
        "panel border swallows hidden buttons"
    );
    assert!(matches!(
        hit_test(&layout, &app, 25, 5),
        Some(Action::SetVolume(_))
    ));
    assert!(matches!(
        hit_test(&layout, &app, 25, 6),
        Some(Action::SeekToFraction(_))
    ));

    let sliding = UiLayout {
        playlist_rect: rect(0, 0, 12, 8),
        playlist_list_inner: Rect::default(),
        ..layout
    };
    assert_eq!(hit_test(&sliding, &app, 5, 5), None);
    assert!(matches!(
        hit_test(&sliding, &app, 15, 5),
        Some(Action::SetVolume(_))
    ));
}

/// 命中宽度按显示宽度算：中日韩名字一个字占两格。
#[test]
fn artist_hit_region_uses_display_width() {
    let layout = UiLayout {
        info_meta: rect(0, 0, 10, 3),
        ..UiLayout::default()
    };
    let mut app = state(Overlay::None);
    app.player.track.artist = "周杰伦".to_string();

    assert_eq!(
        hit_test(&layout, &app, 5, 1),
        Some(Action::OpenAuthorPage(0))
    );
    assert_eq!(hit_test(&layout, &app, 6, 1), None, "名字只有 6 格宽");
}

/// 作者行没画出来（meta 不够高 / 名字为空）时不登记命中区。
#[test]
fn artist_hit_region_is_absent_when_the_row_is_not_drawn() {
    let mut app = state(Overlay::None);
    app.player.track.artist = "Jay".to_string();

    let one_row = UiLayout {
        info_meta: rect(2, 5, 20, 1),
        ..UiLayout::default()
    };
    assert_eq!(hit_test(&one_row, &app, 2, 6), None, "只有标题行");

    let three_rows = UiLayout {
        info_meta: rect(2, 5, 20, 3),
        ..UiLayout::default()
    };
    assert_eq!(
        hit_test(&three_rows, &app, 2, 6),
        Some(Action::OpenAuthorPage(0))
    );

    app.player.track.artist.clear();
    assert_eq!(
        hit_test(&three_rows, &app, 2, 6),
        None,
        "空名字不登记命中区"
    );

    app.player.track.artist = "Jay".to_string();
    assert_eq!(
        hit_test(&UiLayout::default(), &app, 0, 1),
        None,
        "整个信息区没画时零矩形不命中"
    );
}

/// 设置弹窗：命中条目行 → ModalSelect(序号)，弹窗内其余位置仍被吞掉。
#[test]
fn settings_modal_rows_are_clickable() {
    let mut layout = page_layout();
    layout.modal_rows.push(rect(2, 10, 20, 1), 0);
    layout.modal_rows.push(rect(2, 11, 20, 1), 1);
    layout.modal_rows.push(rect(2, 12, 20, 1), 2);

    let app = state(Overlay::SettingsModal);

    assert_eq!(hit_test(&layout, &app, 5, 10), Some(Action::ModalSelect(0)));
    assert_eq!(hit_test(&layout, &app, 5, 11), Some(Action::ModalSelect(1)));
    assert_eq!(hit_test(&layout, &app, 5, 12), Some(Action::ModalSelect(2)));
    assert_eq!(hit_test(&layout, &app, 5, 13), None, "行外不命中");
    assert_eq!(hit_test(&layout, &app, 0, 1), None, "底层进度条仍被吞掉");
}

/// 空矩形与超上限的行不登记（渲染本来就画不出来）。
#[test]
fn modal_rows_ignore_degenerate_and_overflowing_rows() {
    let mut rows = ModalRows::default();
    rows.push(Rect::default(), 0);
    assert_eq!(rows.hit(0, 0), None);

    for idx in 0..(ModalRows::MAX + 5) {
        rows.push(rect(0, idx as u16, 10, 1), idx);
    }

    assert_eq!(rows.get(ModalRows::MAX), None, "超上限的行被丢弃");
    assert_eq!(
        rows.get(ModalRows::MAX - 1),
        Some(rect(0, (ModalRows::MAX - 1) as u16, 10, 1))
    );
    assert_eq!(rows.hit(3, 0), Some(0));
}

/// 登记的是条目序号而不是登记次序：滚动过的窗口与固定在底部的行
/// 都要能按条目号取回（否则点击会落到别的条目上）。
#[test]
fn modal_rows_key_on_item_index_not_registration_order() {
    let mut rows = ModalRows::default();
    rows.push(rect(0, 5, 10, 1), 7);
    rows.push(rect(0, 6, 10, 1), 8);
    rows.push(rect(0, 9, 10, 1), 11);

    assert_eq!(rows.hit(1, 5), Some(7), "窗口首行回的是条目序号");
    assert_eq!(rows.hit(1, 6), Some(8));
    assert_eq!(rows.hit(1, 9), Some(11));
    assert_eq!(rows.get(7), Some(rect(0, 5, 10, 1)));
    assert_eq!(rows.get(11), Some(rect(0, 9, 10, 1)));
    assert_eq!(rows.get(0), None, "没画出来的条目不登记");
    assert_eq!(rows.get(10), None);
}

/// 播放列表命中区必须用渲染的虚拟滚动窗口：滚过一屏后点到的仍是看到的那首，
/// 末尾两行 footer 不命中。
#[test]
fn playlist_click_uses_the_render_window() {
    let layout = UiLayout {
        playlist_list_inner: rect(0, 10, 30, 6),
        ..UiLayout::default()
    };
    let mut app = state(Overlay::Playlist);
    app.playlist_list_scroll = 7;
    app.playlist_list_rows = 4;

    assert_eq!(
        hit_test(&layout, &app, 1, 10),
        Some(Action::PlaylistSelect(7)),
        "首行对应窗口起点"
    );
    assert_eq!(
        hit_test(&layout, &app, 1, 13),
        Some(Action::PlaylistSelect(10))
    );
    assert_eq!(hit_test(&layout, &app, 1, 14), None, "footer 行不命中");
    assert_eq!(hit_test(&layout, &app, 1, 15), None, "footer 行不命中");
}

/// 滚轮只在播放列表面板内生效（面板未打开时不响应）。
#[test]
fn wheel_scrolls_only_over_the_playlist_panel() {
    let layout = UiLayout {
        playlist_rect: rect(2, 5, 30, 20),
        ..UiLayout::default()
    };

    let open = state(Overlay::Playlist);
    assert!(wheel_over_playlist(&layout, &open, 10, 10));
    assert!(!wheel_over_playlist(&layout, &open, 40, 10), "面板外不响应");
    assert!(
        !wheel_over_playlist(&layout, &state(Overlay::None), 10, 10),
        "面板未打开时不响应"
    );
}

/// 音量拖动与点击用同一条换算（否则按住拖会和点一下的落点不一致）。
#[test]
fn volume_at_matches_the_volume_bar() {
    let layout = UiLayout {
        info_volume: rect(0, 2, 12, 1),
        ..UiLayout::default()
    };
    assert_eq!(volume_at(&layout, 0, 2), Some(0.0));
    assert_eq!(volume_at(&layout, 11, 2), Some(1.0));
    assert_eq!(volume_at(&layout, 6, 2), Some(0.5));
    assert_eq!(volume_at(&layout, 12, 2), None, "条外不响应");
    assert_eq!(volume_at(&layout, 6, 3), None, "行外不响应");
}

/// 拖动一旦从条内开始就不再要求光标留在条上：拖出左右边界按端点钳制，
/// 上下拖出行也照样生效。
#[test]
fn volume_drag_clamps_outside_the_bar() {
    let layout = UiLayout {
        info_volume: rect(0, 2, 12, 1),
        ..UiLayout::default()
    };

    assert_eq!(volume_for_drag(&layout, 0), Some(0.0));
    assert_eq!(volume_for_drag(&layout, 11), Some(1.0));
    assert_eq!(
        volume_for_drag(&layout, 200),
        Some(1.0),
        "拖到右边之外仍生效"
    );
    assert_eq!(volume_for_drag(&layout, 6), Some(0.5));

    // 音量条没有绘制（宽度退化）时不改音量。
    let empty = UiLayout::default();
    assert_eq!(volume_for_drag(&empty, 3), None);
}

#[test]
fn progress_drag_matches_click_geometry_and_clamps_outside_track() {
    let layout = UiLayout {
        info_progress: rect(10, 6, 11, 1),
        ..UiLayout::default()
    };
    let app = state(Overlay::None);
    for (col, ratio) in [(10, 0.0), (15, 0.5), (20, 1.0)] {
        assert_eq!(
            hit_test(&layout, &app, col, 6),
            Some(Action::SeekToFraction(ratio))
        );
        assert_eq!(progress_for_drag(&layout, col), Some(ratio));
    }
    assert_eq!(hit_test(&layout, &app, 15, 5), None);
    assert_eq!(hit_test(&layout, &app, 21, 6), None);
    assert_eq!(progress_for_drag(&layout, 0), Some(0.0));
    assert_eq!(progress_for_drag(&layout, u16::MAX), Some(1.0));
}

#[test]
fn progress_drag_ignores_missing_or_degenerate_tracks() {
    let app = state(Overlay::None);
    for track in [Rect::default(), rect(10, 6, 1, 1), rect(10, 6, 11, 0)] {
        let layout = UiLayout {
            info_progress: track,
            ..UiLayout::default()
        };
        assert_eq!(progress_for_drag(&layout, 10), None);
        assert_eq!(hit_test(&layout, &app, 10, 6), None);
    }
}

/// 按键提示弹窗的条目数必须由列表本身决定：写死会让末尾几行选不中
/// （历史上常量 14 对不上 16 行就是这么来的），小窗口切换也已从设置弹窗搬到这里。
#[test]
fn help_items_cover_every_row_and_include_the_moved_hint() {
    let app = state(Overlay::HelpModal);
    let items = help_items(&app);

    assert_eq!(help_item_count(&app), items.len(), "翻页计数与列表同源");
    assert!(items.len() > 14, "条目数应随列表增长，不再写死");
    assert!(
        items.iter().any(|(label, _)| label.contains("小窗口")),
        "“小窗口切换显示”应从设置弹窗移到按键提示里"
    );
}

/// 读回一行的实际渲染文本（用 TestBackend 的缓冲，等价于看画面）。
fn line_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
    let area = *buf.area();
    (area.x..area.x + area.width)
        .map(|x| buf[(x, y)].symbol())
        .collect()
}

/// 去掉空白后再比较：宽字符（CJK）在缓冲里会占一个额外空位。
fn compact(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn render_to_buffer(
    app: &mut AppState,
    draw: impl FnOnce(&mut ratatui::Frame, &mut AppState, &mut ModalRows),
) -> (ModalRows, ratatui::buffer::Buffer) {
    render_to_buffer_sized(80, 24, app, draw)
}

/// 指定尺寸渲染：矮终端下弹窗条目区会被截断，命中区必须跟着截断。
fn render_to_buffer_sized(
    width: u16,
    height: u16,
    app: &mut AppState,
    draw: impl FnOnce(&mut ratatui::Frame, &mut AppState, &mut ModalRows),
) -> (ModalRows, ratatui::buffer::Buffer) {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    let mut rows = ModalRows::default();
    terminal.draw(|f| draw(f, app, &mut rows)).expect("draw");
    let buf = terminal.backend().buffer().clone();
    (rows, buf)
}

/// 歌词浮窗弹窗：命中行必须落在真正画着该条目的那一行上
/// （否则就是"看得见点不动"或"点到的不是看到的"）。
#[test]
fn fullscreen_lyrics_use_word_colors_and_translation() {
    use crate::data::config::VisualizeMode;
    use crate::lyrics::{LyricLine, LyricWord};
    use crate::tmplayer::app::state::PlaybackState;
    use std::time::Duration;
    use std::time::Instant;

    let mut app = state(Overlay::None);
    app.config.show_hints = false;
    app.config.small_window_display = false;
    app.config.visualize = VisualizeMode::Lyrics;
    app.config.word_lyrics = true;
    app.config.lyric_translation = true;
    app.language = crate::data::config::Language::En;
    app.player.playback = PlaybackState::Playing;
    app.player.position = Duration::from_millis(1_700);
    app.position_anchor_at = Instant::now();
    app.player.track.lyrics = Some(vec![
        LyricLine {
            start_ms: 1_000,
            end_ms: Some(2_000),
            text: "你好".into(),
            words: vec![
                LyricWord {
                    text: "你".into(),
                    start_ms: 1_000,
                    end_ms: 1_400,
                },
                LyricWord {
                    text: "好".into(),
                    start_ms: 1_400,
                    end_ms: 2_000,
                },
            ],
            translation: Some("hello".into()),
        },
        LyricLine {
            start_ms: 3_000,
            end_ms: Some(4_000),
            text: "下一句".into(),
            words: Vec::new(),
            translation: None,
        },
    ]);

    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    let mut covers = CoverPipeline::new(WakeSignal::default());
    let mut drawer = PlaylistDrawer::default();
    draw_page(&mut terminal, &mut app, &mut covers, &mut drawer).unwrap();
    let buffer = terminal.backend().buffer();

    let cell = |symbol: &str| {
        (0..24)
            .find_map(|y| {
                (0..80).find_map(|x| (buffer[(x, y)].symbol() == symbol).then_some((x, y)))
            })
            .unwrap_or_else(|| panic!("{symbol:?} not drawn"))
    };

    let current = cell("你");
    let future = cell("下");
    assert_ne!(
        buffer[current].fg,
        ratatui::style::Color::Rgb(128, 128, 128),
        "current word must not use the inactive color"
    );
    assert_eq!(
        buffer[future].fg,
        ratatui::style::Color::Rgb(128, 128, 128),
        "future line uses subtext"
    );
    let translation = cell("h");
    assert!(
        buffer[translation].fg != ratatui::style::Color::Reset,
        "translation row is painted"
    );
}

#[test]
fn lyrics_modal_hit_rows_sit_on_the_drawn_rows() {
    let mut app = state(Overlay::LyricsSettingsModal);
    let (rows, buf) = render_to_buffer(&mut app, |f, app, rows| {
        render_lyrics_settings_modal(f, f.area(), app, rows)
    });

    let expected = [
        "歌词浮窗:",
        "歌词浮窗拖动:",
        "歌词浮窗边缘吸附:",
        "逐字歌词:",
        "歌词翻译:",
    ];
    assert_eq!(rows.len(), expected.len());

    for (idx, label) in expected.iter().enumerate() {
        let rect = rows.get(idx).expect("登记了命中行");
        let text = line_text(&buf, rect.y);
        assert!(
            compact(&text).contains(&compact(label)),
            "第 {idx} 行画的是 {text:?}"
        );
    }
}

/// 按键提示弹窗：命中行落在条目行上，且移到这里的“小窗口切换显示”能显示出来。
#[test]
fn help_modal_hit_rows_sit_on_the_drawn_rows() {
    let mut app = state(Overlay::HelpModal);
    let items = help_items(&app);
    let moved = items
        .iter()
        .position(|(label, _)| label.contains("小窗口"))
        .expect("小窗口条目");
    // 选中它，渲染窗口就会滚到它，从而一定在命中区内。
    app.help_keybind_selected = moved;

    let (rows, buf) = render_to_buffer(&mut app, |f, app, rows| {
        render_help_modal(f, f.area(), app, rows)
    });

    assert!(rows.len() > 0, "条目行要登记出来才能点");
    let mut saw_hint = false;
    for idx in 0..rows.len() {
        let rect = rows.get(idx).expect("命中行");
        let text = compact(&line_text(&buf, rect.y));
        assert!(
            items
                .iter()
                .any(|(label, key)| text.contains(&compact(label)) && text.contains(&compact(key))),
            "命中行落在非条目行上: {text:?}"
        );
        saw_hint |= text.contains(&compact("小窗口"));
    }
    assert!(saw_hint, "“小窗口切换显示”应出现在按键提示弹窗里");
}

/// 矮终端下 about 仍按条目序号 8 登记，不能映射到被截断的其他条目。
#[test]
fn settings_modal_about_row_keeps_its_item_index_when_items_are_truncated() {
    let mut app = state(Overlay::SettingsModal);
    let (rows, buf) = render_to_buffer_sized(80, 12, &mut app, |f, app, rows| {
        render_settings_modal(f, f.area(), app, rows)
    });

    let about = rows.get(8).expect("about 行按条目序号 8 登记");
    assert!(
        compact(&line_text(&buf, about.y)).contains("about"),
        "8 号条目行画的是 about"
    );
    assert!(rows.len() < 9, "12 行终端下条目区放不下 8 条");

    let mut layout = page_layout();
    layout.modal_rows = rows;
    assert_eq!(
        hit_test(&layout, &app, about.x, about.y),
        Some(Action::ModalSelect(8)),
        "点 about 行要回条目序号 8"
    );
}

/// 按键提示弹窗滚动后，每一行的命中序号必须等于画在该行的条目序号
/// （否则点“第 2 行显示的那条”会选中上一条）。
#[test]
fn help_modal_hit_index_matches_the_row_after_scrolling() {
    let mut app = state(Overlay::HelpModal);
    let items = help_items(&app);
    // 选末条：24 行终端放不下 17 条，渲染窗口必然滚动，滚动量 = 1。
    app.help_keybind_selected = items.len() - 1;

    let (rows, buf) = render_to_buffer(&mut app, |f, app, rows| {
        render_help_modal(f, f.area(), app, rows)
    });

    let mut hits = 0;
    for y in 0..buf.area().height {
        let Some(index) = (0..buf.area().width).find_map(|x| rows.hit(x, y)) else {
            continue;
        };
        let text = compact(&line_text(&buf, y));
        let (label, key) = &items[index];
        assert!(
            text.contains(&compact(label)) && text.contains(&compact(key)),
            "第 {y} 行的命中序号 {index} 与画面 {text:?} 不符"
        );
        hits += 1;
    }
    assert_eq!(hits, rows.len(), "登记的行都该画在屏幕上");
    assert!(hits > 0, "至少要有一行可点");
}

/// 与主应用一致的越界聚焦滚动：焦点还在窗口内时视口不动，
/// 越过下/上边界才滚（旧实现会把焦点持续钉在窗口底边）。
#[test]
fn help_modal_viewport_stays_put_while_focus_is_inside() {
    let mut app = state(Overlay::HelpModal);
    let items = help_items(&app);
    let last = items.len() - 1;

    // 末条越界：视口滚起来（24 行终端放不下全部条目）。
    app.help_keybind_selected = last;
    let _ = render_to_buffer(&mut app, |f, app, rows| {
        render_help_modal(f, f.area(), app, rows)
    });
    let scrolled = app.help_keybind_scroll;
    assert!(scrolled > 0, "末条应当把视口滚起来");

    // 倒数第二条仍在窗口内：视口不得回跳。
    app.help_keybind_selected = last - 1;
    let _ = render_to_buffer(&mut app, |f, app, rows| {
        render_help_modal(f, f.area(), app, rows)
    });
    assert_eq!(app.help_keybind_scroll, scrolled, "窗口内不动");

    // 回首条：越过上边界，视口回到顶。
    app.help_keybind_selected = 0;
    let _ = render_to_buffer(&mut app, |f, app, rows| {
        render_help_modal(f, f.area(), app, rows)
    });
    assert_eq!(app.help_keybind_scroll, 0, "越过顶边回到顶部");
}

fn draw_page_reveal<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut AppState,
    covers: &mut CoverPipeline,
    sidebar: &mut PlaylistDrawer,
    progress: f32,
) -> std::result::Result<UiLayout, B::Error> {
    draw_page_reveal_with_snapshot(terminal, app, covers, sidebar, None, progress)
}

#[test]
fn general_modal_scrolling_preserves_viewport_and_drawn_hit_rows() {
    let mut app = state(Overlay::GeneralSettingsModal);
    app.config.mpris_name = " \t ".into();
    app.general_settings_selected = 5;
    let (rows, buf) = render_to_buffer_sized(80, 10, &mut app, |f, app, rows| {
        render_general_settings_modal(f, f.area(), app, rows)
    });
    let name_row = rows.get(5).expect("focused name remains visible");
    assert!(compact(&line_text(&buf, name_row.y)).contains("MPRIS服务名称:CNMPlayer"));
    let scroll = app.general_settings_scroll;
    assert!(scroll > 0);
    let layout = UiLayout {
        modal_rows: rows,
        ..UiLayout::default()
    };
    assert_eq!(
        hit_test(&layout, &app, name_row.x, name_row.y),
        Some(Action::ModalSelect(5))
    );
    app.general_settings_selected = 4;
    render_to_buffer_sized(80, 10, &mut app, |f, app, rows| {
        render_general_settings_modal(f, f.area(), app, rows)
    });
    assert_eq!(app.general_settings_scroll, scroll);
    app.general_settings_selected = 0;
    render_to_buffer_sized(80, 10, &mut app, |f, app, rows| {
        render_general_settings_modal(f, f.area(), app, rows)
    });
    assert_eq!(app.general_settings_scroll, 0);
}

#[test]
fn empty_name_edit_draws_value_only_caret_and_ignores_row_clicks() {
    for language in [
        crate::data::config::Language::Zh,
        crate::data::config::Language::En,
    ] {
        let mut app = state(Overlay::MprisNameEditModal);
        app.language = language;
        app.general_settings_selected = 5;
        app.mpris_name_edit = Some(crate::app::settings_controller::SettingsTextEdit::new(
            String::new(),
        ));
        let (rows, buf) = render_to_buffer_sized(80, 24, &mut app, |f, app, rows| {
            render_general_settings_modal(f, f.area(), app, rows)
        });
        let row = rows.get(5).unwrap();
        let prefix = if language == crate::data::config::Language::Zh {
            "  MPRIS服务名称: "
        } else {
            "  MPRIS Service Name: "
        };
        let caret_x = row.x + crate::ui::settings::display_width(prefix) as u16;
        assert_eq!(
            buf[(row.x + 2, row.y)].style().bg,
            Some(app.theme.color_surface())
        );
        let caret = &buf[(caret_x, row.y)];
        assert_eq!(caret.symbol(), " ");
        assert_eq!(caret.style().bg, Some(app.theme.color_buff()));
        assert!(
            caret
                .style()
                .add_modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
        let layout = UiLayout {
            modal_rows: rows,
            ..UiLayout::default()
        };
        assert_eq!(hit_test(&layout, &app, row.x, row.y), None);
    }
}

#[test]
fn long_chinese_name_edit_scrolls_only_when_caret_crosses_value_boundary() {
    let mut app = state(Overlay::MprisNameEditModal);
    app.general_settings_selected = 5;
    app.mpris_name_edit = Some(crate::app::settings_controller::SettingsTextEdit::new(
        "当前曲名".repeat(30),
    ));
    let (rows, buf) = render_to_buffer_sized(80, 24, &mut app, |f, app, rows| {
        render_general_settings_modal(f, f.area(), app, rows)
    });
    let row = rows.get(5).unwrap();
    let value_x = row.x + crate::ui::settings::display_width("  MPRIS服务名称: ") as u16;
    let scroll = app.mpris_name_edit.as_ref().unwrap().window_col;
    assert!(scroll > 0);
    assert!((value_x..row.x + row.width).any(|x| {
        buf[(x, row.y)]
            .style()
            .add_modifier
            .contains(ratatui::style::Modifier::REVERSED)
    }));
    app.mpris_name_edit.as_mut().unwrap().move_cursor(-1);
    render_to_buffer_sized(80, 24, &mut app, |f, app, rows| {
        render_general_settings_modal(f, f.area(), app, rows)
    });
    assert_eq!(app.mpris_name_edit.as_ref().unwrap().window_col, scroll);
    app.mpris_name_edit.as_mut().unwrap().home();
    let (rows, buf) = render_to_buffer_sized(80, 24, &mut app, |f, app, rows| {
        render_general_settings_modal(f, f.area(), app, rows)
    });
    assert_eq!(app.mpris_name_edit.as_ref().unwrap().window_col, 0);
    let row = rows.get(5).unwrap();
    assert_eq!(buf[(value_x, row.y)].symbol(), "当");
    assert!(
        buf[(value_x, row.y)]
            .style()
            .add_modifier
            .contains(ratatui::style::Modifier::REVERSED)
    );
}
