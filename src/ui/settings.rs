use crate::app::{App, HitRect, Overlay};
use crate::data::config::{AudioQuality, BarChannels, BarNumber, Language, VisualizeMode};
use crate::tmplayer::data::about::{BrailleImage, about_info};
use crate::tmplayer::ui::borders::SOLID_BORDER;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthChar;

pub fn draw_settings_modal(frame: &mut Frame, app: &mut App) {
    let size = frame.area();

    // 命中区每帧重注册：页面切换/滚动变化后不该留下上一页的矩形。
    app.clear_settings_item_hits();

    if matches!(app.overlay, Some(Overlay::SettingsAbout)) {
        draw_about_modal(frame, app, size);
        return;
    }

    let area = centered_rect(70, 20, size);

    frame.render_widget(Clear, area);

    let title = match app.overlay {
        Some(Overlay::Settings) => l(app, " 设置 ", " Settings "),
        Some(Overlay::SettingsPlayback) => l(app, " 播放设置 ", " Playback Settings "),
        Some(Overlay::SettingsKeybinds) => l(app, " 按键绑定 ", " Keybinds "),
        Some(Overlay::SettingsLyrics) => l(app, " 歌词浮窗 ", " Lyrics Overlay "),
        Some(Overlay::SettingsDownload) => l(app, " 下载设置 ", " Download Settings "),
        Some(Overlay::SettingsAbout) => " about ",
        _ => l(app, " 设置 ", " Settings "),
    };

    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_set(SOLID_BORDER)
            .title(title)
            .border_style(Style::default().fg(app.theme.color_subtext()))
            .style(base_bg_style(app)),
        area,
    );

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });

    match app.overlay {
        Some(Overlay::SettingsPlayback) => draw_playback_settings(frame, app, inner),
        Some(Overlay::SettingsKeybinds) => draw_keybind_settings(frame, app, inner),
        Some(Overlay::SettingsLyrics) => draw_lyrics_settings(frame, app, inner),
        Some(Overlay::SettingsDownload) => draw_download_settings(frame, app, inner),
        _ => draw_root_settings(frame, app, inner),
    }
}

fn draw_root_settings(frame: &mut Frame, app: &mut App, inner: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.color_surface())),
        rows[0],
    );

    let items = vec![
        format!("{}: {}", l(app, "主题", "Theme"), app.config.theme),
        format!(
            "{}: {}",
            l(app, "背景透明", "Transparent Background"),
            on_off(app, app.config.transparent_background)
        ),
        format!(
            "{}: {}",
            l(app, "语言", "Language"),
            match app.config.language {
                Language::Zh => l(app, "中文", "Chinese"),
                Language::En => "English",
            }
        ),
        format!(
            "{}: {}",
            l(app, "图像协议", "Image Protocol"),
            app.config.graphics_protocol.display_name()
        ),
        format!("{}...", l(app, "播放设置", "Playback Settings")),
        format!("{}...", l(app, "按键绑定", "Keybinds")),
        format!("{}...", l(app, "歌词浮窗", "Lyrics Overlay")),
        format!(
            "{}: {}",
            l(app, "显示提示", "Show Hints"),
            on_off(app, app.config.show_hints)
        ),
        format!(
            "{}: {}",
            l(app, "小窗口显示", "Small Window Display"),
            on_off(app, app.config.small_window_display)
        ),
        format!(
            "{}: {}",
            l(app, "主页更多推荐", "More Home Recommendations"),
            on_off(app, app.config.home_more_recommend)
        ),
        format!("{}...", l(app, "下载设置", "Download Settings")),
        l(app, "退出登录", "Logout").to_string(),
        "about".to_string(),
    ];

    let item_style = |idx: usize| {
        if idx == app.settings_selected {
            Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.color_text())
        }
    };

    // "about" is pinned to the bottom row of the modal; the rest stack from the top.
    let about_idx = items.len().saturating_sub(1);
    let lines: Vec<Line> = items
        .iter()
        .take(about_idx)
        .enumerate()
        .map(|(idx, text)| Line::from(Span::styled(format!("  {}", text), item_style(idx))))
        .collect();

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.color_surface())),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {}", items[about_idx]),
            item_style(about_idx),
        )))
        .style(Style::default().bg(app.theme.color_surface())),
        rows[2],
    );

    // 命中区与上面的渲染同源：前 10 项在 rows[1] 逐行排布（超高的行会被
    // Paragraph 裁掉，故只登记放得下的），about 固定在最底一行。
    for idx in 0..about_idx {
        if idx as u16 >= rows[1].height {
            break;
        }
        app.push_settings_item_hit(
            HitRect {
                x: rows[1].x,
                y: rows[1].y + idx as u16,
                width: rows[1].width,
                height: 1,
            },
            idx,
        );
    }
    if rows[2].height >= 1 && rows[2].width >= 1 {
        app.push_settings_item_hit(
            HitRect {
                x: rows[2].x,
                y: rows[2].y,
                width: rows[2].width,
                height: 1,
            },
            about_idx,
        );
    }
}

fn draw_playback_settings(frame: &mut Frame, app: &mut App, inner: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.color_surface())),
        rows[0],
    );

    let bar_number = match app.config.bar_number {
        BarNumber::Auto => l(app, "自动", "Auto"),
        BarNumber::N16 => "16",
        BarNumber::N32 => "32",
        BarNumber::N48 => "48",
        BarNumber::N64 => "64",
        BarNumber::N80 => "80",
        BarNumber::N96 => "96",
    };

    let channels = match app.config.bar_channels {
        BarChannels::Mono => "Mono",
        BarChannels::Stereo => "Stereo",
    };

    let items = vec![
        format!(
            "{}: {}",
            l(app, "可视化", "Visualization"),
            match app.config.visualize {
                VisualizeMode::Lyrics => l(app, "仅歌词", "Lyrics"),
                VisualizeMode::Hidden => l(app, "关闭", "Off"),
                VisualizeMode::Bars => l(app, "频谱", "Bars"),
                VisualizeMode::Oscilloscope => l(app, "示波器", "Oscilloscope"),
                VisualizeMode::Vector => l(app, "矢量", "Vector"),
            }
        ),
        format!(
            "{}: {}",
            l(app, "超级流畅", "Super Smooth"),
            on_off(app, app.config.super_smooth_bar)
        ),
        format!(
            "{}: {}",
            l(app, "频谱间隔", "Bars Gap"),
            on_off(app, app.config.bars_gap)
        ),
        format!("{}: {}", l(app, "频谱数", "Bars Count"), bar_number),
        format!("{}: {}", l(app, "声道", "Channels"), channels),
        format!(
            "{}: {}",
            l(app, "封面边框", "Cover Border"),
            on_off(app, app.config.album_border)
        ),
        format!(
            "{}: {}",
            l(app, "音质", "Audio Quality"),
            audio_quality_label(app, app.config.audio_quality)
        ),
        format!(
            "{}: {}",
            l(app, "播放记忆", "Playback Memory"),
            on_off(app, app.config.playback_memory)
        ),
    ];

    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(idx, text)| {
            let style = if idx == app.settings_playback_selected {
                Style::default()
                    .fg(app.theme.color_accent2())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.color_text())
            };
            Line::from(Span::styled(format!("  {}", text), style))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.color_surface())),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.color_surface())),
        rows[2],
    );

    // 命中区：与上面的渲染同源的逐行排布（放不下的行不登记）。
    for idx in 0..items.len() {
        if idx as u16 >= rows[1].height {
            break;
        }
        app.push_settings_item_hit(
            HitRect {
                x: rows[1].x,
                y: rows[1].y + idx as u16,
                width: rows[1].width,
                height: 1,
            },
            idx,
        );
    }
}

fn draw_lyrics_settings(frame: &mut Frame, app: &mut App, inner: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.color_surface())),
        rows[0],
    );

    // “边缘吸附”只在拖动开启时可选：关闭时灰置（与播放设置有别，这里明确表达依赖关系）。
    let drag_enabled = app.config.page_lyrics_drag;
    let items = [
        format!(
            "{}: {}",
            l(app, "歌词浮窗", "Lyrics Overlay"),
            on_off(app, app.config.page_lyrics)
        ),
        format!(
            "{}: {}",
            l(app, "歌词浮窗拖动", "Lyrics Overlay Drag"),
            on_off(app, drag_enabled)
        ),
        format!(
            "{}: {}",
            l(app, "歌词浮窗边缘吸附", "Lyrics Overlay Edge Snap"),
            on_off(app, app.config.page_lyrics_snap)
        ),
    ];

    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(idx, text)| {
            let disabled = idx == 2 && !drag_enabled;
            let style = if idx == app.settings_lyrics_selected {
                if disabled {
                    Style::default().fg(app.theme.color_subtext())
                } else {
                    Style::default()
                        .fg(app.theme.color_accent2())
                        .add_modifier(Modifier::BOLD)
                }
            } else if disabled {
                Style::default().fg(app.theme.color_subtext())
            } else {
                Style::default().fg(app.theme.color_text())
            };
            Line::from(Span::styled(format!("  {}", text), style))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.color_surface())),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.color_surface())),
        rows[2],
    );

    // 命中区：与上面的渲染同源的逐行排布（放不下的行不登记）。
    for idx in 0..items.len() {
        if idx as u16 >= rows[1].height {
            break;
        }
        app.push_settings_item_hit(
            HitRect {
                x: rows[1].x,
                y: rows[1].y + idx as u16,
                width: rows[1].width,
                height: 1,
            },
            idx,
        );
    }
}

/// 「下载设置」页：音质 / 下载路径（可进入行内编辑）/ 恢复默认（两段式确认）。
///
/// 下载目录不可用（系统里既没有音乐目录也没有家目录）时，除路径行以外全部灰置且不可选中；
/// 路径行显示 `Null`，手动填一个可写的绝对路径即可恢复下载。
fn draw_download_settings(frame: &mut Frame, app: &mut App, inner: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.color_surface())),
        rows[0],
    );

    let text_color = app.theme.color_text();
    let subtext = app.theme.color_subtext();
    let accent2 = app.theme.color_accent2();
    let warning = app.theme.color_accent3();
    let buff = app.theme.color_buff();
    let surface = app.theme.color_surface();

    let selected = app.settings_download_selected;
    let quality = audio_quality_label(app, app.config.download_audio_quality);
    let path_prefix = format!("{}: ", l(app, "下载路径", "Download Path"));
    let path_display = app.download_display_path();
    let reset_label = if app.download_reset_armed {
        l(app, "确认恢复", "Confirm Restore")
    } else {
        l(app, "恢复默认", "Restore Defaults")
    };

    // 三行的样式先算好（后面要可变借用 `download_path_edit`，不能再借 `app`）。
    // 不可用时只灰置「音质」：路径行与「恢复默认」都留着当出口。
    let row_styles: [Style; crate::app::SETTINGS_DOWNLOAD_ITEMS] = std::array::from_fn(|idx| {
        let disabled = !app.download_settings_enabled() && idx == 0;
        if idx == selected {
            if disabled {
                Style::default().fg(subtext)
            } else {
                Style::default().fg(accent2).add_modifier(Modifier::BOLD)
            }
        } else if disabled {
            Style::default().fg(subtext)
        } else {
            Style::default().fg(text_color)
        }
    });

    let mut lines: Vec<Line> = Vec::with_capacity(crate::app::SETTINGS_DOWNLOAD_ITEMS);
    for idx in 0..crate::app::SETTINGS_DOWNLOAD_ITEMS {
        let style = row_styles[idx];
        let spans: Vec<Span> = match idx {
            0 => vec![Span::styled(
                format!("  {}: {}", l(app, "音质", "Audio Quality"), quality),
                style,
            )],
            1 => {
                let avail = usize::from(rows[1].width)
                    .saturating_sub(display_width(&path_prefix) + 2)
                    .max(1);
                if let Some(edit) = app.download_path_edit.as_mut() {
                    // 编辑态：只有**路径值**这一段的底色变 buff（标签保持行样式），
                    // 光标所在字符反显；窗口只在光标撞到边界时才横向滚动。
                    let caret_col = caret_display_col(&edit.buffer, edit.cursor);
                    edit.window_col = adjust_path_window(edit.window_col, caret_col, avail);
                    let (visible, caret) =
                        path_window(&edit.buffer, edit.cursor, avail, edit.window_col);
                    let value_style = Style::default().fg(text_color).bg(buff);
                    let caret_style = value_style.add_modifier(Modifier::REVERSED);
                    let head: String = visible.chars().take(caret).collect();
                    let caret_char = visible
                        .chars()
                        .nth(caret)
                        .map(|ch| ch.to_string())
                        .unwrap_or_else(|| " ".to_string());
                    let tail: String = visible.chars().skip(caret + 1).collect();
                    vec![
                        // 标签保持 modal 底色（只有路径值段落换成 buff）。
                        Span::styled(format!("  {path_prefix}"), style.bg(surface)),
                        Span::styled(head, value_style),
                        Span::styled(caret_char, caret_style),
                        Span::styled(tail, value_style),
                    ]
                } else {
                    let clipped = clip_to_display_width(&path_display, avail);
                    vec![Span::styled(format!("  {path_prefix}{clipped}"), style)]
                }
            }
            _ => {
                let style = if app.download_reset_armed {
                    Style::default().fg(warning).add_modifier(Modifier::BOLD)
                } else {
                    style
                };
                vec![Span::styled(format!("  {reset_label}"), style)]
            }
        };
        lines.push(Line::from(spans));
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(app.theme.color_surface())),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.color_surface())),
        rows[2],
    );

    // 命中区：与渲染同源的逐行排布（放不下的行不登记）。
    for idx in 0..crate::app::SETTINGS_DOWNLOAD_ITEMS {
        if idx as u16 >= rows[1].height {
            break;
        }
        app.push_settings_item_hit(
            HitRect {
                x: rows[1].x,
                y: rows[1].y + idx as u16,
                width: rows[1].width,
                height: 1,
            },
            idx,
        );
    }
}

/// 光标所在的显示列（按字符宽度累计）。
pub(crate) fn caret_display_col(text: &str, cursor: usize) -> usize {
    text.chars()
        .take(cursor)
        .map(|ch| ch.width().unwrap_or(0))
        .sum()
}

/// 编辑态窗口的左边界：**只在光标撞到边界时**挪动，且**一次只挪一个字符**。
///
/// 光标在窗口内 → 不动（文本不跟着每一次光标移动一起滑）；
/// 撞右边界 → 窗口右移一格、光标贴回右端；撞左边界 → 窗口左移一格；
/// 一次性跳很远（Home / End / 点击）时直接对齐，保证光标落在窗口内。
pub(crate) fn adjust_path_window(window: usize, caret_col: usize, width: usize) -> usize {
    if width == 0 {
        return 0;
    }

    if caret_col < window {
        caret_col
    } else if caret_col >= window + width {
        (caret_col + 1).saturating_sub(width)
    } else {
        window
    }
}

/// 路径行的可见窗口：`(窗口串, 光标在窗口内的字符下标)`；`start_col` 是窗口左边界的显示列。
pub(crate) fn path_window(
    text: &str,
    cursor: usize,
    width: usize,
    start_col: usize,
) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    if width == 0 {
        return (String::new(), 0);
    }

    let widths: Vec<usize> = chars.iter().map(|ch| ch.width().unwrap_or(0)).collect();
    let mut out = String::new();
    let mut col = 0usize;
    let mut used = 0usize;
    let mut caret = 0usize;
    for (idx, ch) in chars.iter().enumerate() {
        let w = widths[idx];
        if col + w <= start_col {
            col += w;
            continue;
        }
        if used + w > width {
            break;
        }
        if idx == cursor {
            caret = out.chars().count();
        }
        out.push(*ch);
        used += w;
        col += w;
    }
    if cursor >= chars.len() {
        caret = out.chars().count();
    }
    (out, caret)
}

pub(crate) fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

pub(crate) fn clip_to_display_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > max_width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}
fn draw_keybind_settings(frame: &mut Frame, app: &mut App, inner: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.color_surface())),
        rows[0],
    );

    let mut lines: Vec<Line> = (0..crate::app::SETTINGS_KEYBIND_ITEMS)
        .map(|idx| {
            let is_rebinding = app.settings_keybind_rebinding == Some(idx);
            let style = if is_rebinding {
                Style::default()
                    .fg(app.theme.color_accent())
                    .add_modifier(Modifier::BOLD)
            } else if idx == app.settings_keybind_selected {
                Style::default()
                    .fg(app.theme.color_accent2())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.color_text())
            };

            let mut label = app.keybind_label_for_index(idx);
            if is_rebinding {
                label.push_str(l(app, "  [等待输入]", "  [Waiting Input]"));
            }

            Line::from(Span::styled(format!("  {}", label), style))
        })
        .collect();

    lines.push(Line::from(Span::styled(
        format!(
            "  {}",
            l(
                app,
                "侧边栏歌单区切换（Ctrl+up/down）",
                "Sidebar Playlist Section Switch (Ctrl+Up/Down)",
            )
        ),
        Style::default().fg(app.theme.color_subtext()),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "  {}",
            l(app, "按键绑定弹窗（Ctrl+K）", "Open Keybinds (Ctrl+K)")
        ),
        Style::default().fg(app.theme.color_subtext()),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "  {}",
            l(
                app,
                "重置快捷键（Ctrl+Alt+R）",
                "Reset Keybinds (Ctrl+Alt+R)"
            )
        ),
        Style::default().fg(app.theme.color_subtext()),
    )));

    let focus_index = app
        .settings_keybind_rebinding
        .unwrap_or(app.settings_keybind_selected);
    let visible_rows = rows[1].height as usize;
    let total_rows = lines.len();
    let scroll = scroll_for_focus(total_rows, visible_rows, focus_index);

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(app.theme.color_surface()))
            .scroll((scroll as u16, 0)),
        rows[1],
    );

    let hint = if let Some(index) = app.settings_keybind_rebinding {
        format!(
            "{}: {}  {}",
            l(app, "正在重绑", "Rebinding"),
            app.keybind_label_for_index(index),
            l(
                app,
                "按下新快捷键，Esc 取消",
                "Press a new shortcut, Esc to cancel"
            )
        )
    } else {
        l(
            app,
            "Enter 重绑  Ctrl+Alt+R 重置  Esc 返回",
            "Enter rebind  Ctrl+Alt+R reset  Esc back",
        )
        .to_string()
    };

    frame.render_widget(
        Paragraph::new(hint)
            .style(
                Style::default()
                    .fg(app.theme.color_subtext())
                    .bg(app.theme.color_surface()),
            )
            .wrap(ratatui::widgets::Wrap { trim: true }),
        rows[2],
    );

    // 命中区：只登记窗口内可见的快捷键行（渲染带 `scroll` 偏移；末尾三行
    // 是说明文字、不属于任何条目）。
    for (idx, offset) in scrolled_rows(crate::app::SETTINGS_KEYBIND_ITEMS, visible_rows, scroll) {
        app.push_settings_item_hit(
            HitRect {
                x: rows[1].x,
                y: rows[1].y + offset,
                width: rows[1].width,
                height: 1,
            },
            idx,
        );
    }
}

/// 让 `focus` 落在可视窗口内的滚动偏移（渲染与命中区共用同一算法）。
fn scroll_for_focus(total: usize, visible_rows: usize, focus: usize) -> usize {
    if visible_rows == 0 || focus < visible_rows {
        return 0;
    }

    (focus + 1 - visible_rows).min(total.saturating_sub(visible_rows))
}

/// 窗口内可见的条目：(条目序号, 相对行号)。渲染带同一 `scroll`，
/// 因此这份映射就是"第几行画的是第几个条目"。
fn scrolled_rows(total: usize, visible_rows: usize, scroll: usize) -> Vec<(usize, u16)> {
    if visible_rows == 0 {
        return Vec::new();
    }

    (scroll.min(total)..total)
        .take(visible_rows)
        .enumerate()
        .map(|(offset, idx)| (idx, offset as u16))
        .collect()
}

fn draw_about_modal(frame: &mut Frame, app: &mut App, size: Rect) {
    let area = centered_rect(70, 22, size);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(SOLID_BORDER)
        .title(" about ")
        .style(base_bg_style(app));
    frame.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });

    #[cfg(feature = "easter-egg")]
    let phase = app.about_egg.phase;
    #[cfg(feature = "easter-egg")]
    let mascot_active = phase == crate::app::EasterEggPhase::Active;
    #[cfg(not(feature = "easter-egg"))]
    let mascot_active = false;

    // 迸发期间扫过线之上已经是彩蛋内容，因此那一段也走形象分支。
    if mascot_active {
        #[cfg(feature = "easter-egg")]
        draw_about_mascot(frame, app, inner);
    } else {
        #[cfg(feature = "easter-egg")]
        if phase == crate::app::EasterEggPhase::Bursting {
            draw_about_mascot(frame, app, inner);
        } else {
            draw_about_static(frame, app, inner);
        }
        #[cfg(not(feature = "easter-egg"))]
        draw_about_static(frame, app, inner);

        // 蓄力的逐格填充与迸发的扫过都压在内容之上。
        #[cfg(feature = "easter-egg")]
        draw_about_noise(frame, app, inner);
    }

    let y = area.y + area.height.saturating_sub(1);
    let version_area = Rect {
        x: area.x.saturating_add(1),
        y,
        width: area.width.saturating_sub(2),
        height: 1,
    };

    #[cfg(feature = "easter-egg")]
    {
        app.about_egg.version_hit = Some(crate::app::HitRect {
            x: version_area.x,
            y: version_area.y,
            width: version_area.width,
            height: version_area.height,
        });
    }

    draw_about_version(frame, app, version_area);

    // 彩蛋激活后边框（含底部文字所在的那一行）逆时针循环流动噪点色。
    #[cfg(feature = "easter-egg")]
    if mascot_active {
        draw_about_border_flow(frame, app, area);
    }
}

/// 未触发彩蛋时的 about 内容：左侧点阵形象 + 右侧文本。
fn draw_about_static(frame: &mut Frame, app: &App, inner: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(inner);

    draw_about_braille(frame, app, chunks[0]);
    draw_about_text(frame, app, chunks[1]);
}

/// 底部的版本信息行。激活后换成形象的表情。
fn draw_about_version(frame: &mut Frame, app: &App, area: Rect) {
    #[cfg(feature = "easter-egg")]
    if app.about_egg.phase == crate::app::EasterEggPhase::Active {
        use crate::render::mascot;

        frame.render_widget(
            Paragraph::new(mascot::MASCOT_CAPTION)
                .alignment(Alignment::Center)
                .style(
                    Style::default()
                        .fg(app.theme.color_accent2())
                        .bg(app.theme.color_surface()),
                ),
            area,
        );
        return;
    }

    let info = about_info();
    frame.render_widget(
        Paragraph::new(format!("v{}", info.version))
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(app.theme.color_subtext())
                    .bg(app.theme.color_surface()),
            ),
        area,
    );
}

/// 内容区的噪点：蓄力期逐格随机填满，填满后静止，迸发期自下向上消失。
///
/// 两阶段共用一套逐格绘制：蓄力决定“哪些格已填充”，迸发决定“哪些行还没被扫掉”。
#[cfg(feature = "easter-egg")]
fn draw_about_noise(frame: &mut Frame, app: &App, inner: Rect) {
    use crate::app::EasterEggPhase;
    use crate::render::mascot;
    use ratatui::style::Style;

    /// 噪点每隔这么久换一次配色：蓄力期借此闪动，迸发期冻结不再变化。
    const NOISE_STEP_MS: u64 = 60;

    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let Some(started_at) = app.about_egg.phase_started_at else {
        return;
    };
    let elapsed = started_at.elapsed();

    // sweep_line：扫过线。此行及其下方的噪点已被扫掉，露出下面的彩蛋内容；
    // 线以上仍是噪点。线从内容区下沿起步、向上推进，因此消失是自下向上的。
    let (progress, sweep_line, seed) = match app.about_egg.phase {
        EasterEggPhase::Charging => {
            let p = (elapsed.as_secs_f32() / mascot::CHARGE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
            let seed = elapsed.as_millis() as u64 / NOISE_STEP_MS;
            (p, inner.y + inner.height, seed)
        }
        EasterEggPhase::Bursting => {
            let t = (elapsed.as_secs_f32() / mascot::BURST_DURATION.as_secs_f32()).clamp(0.0, 1.0);
            let swept = (t * inner.height as f32).round() as u16;
            // 冻结在蓄力最后一帧的配色上，填满之后噪点便不再闪动。
            let seed = mascot::CHARGE_DURATION.as_millis() as u64 / NOISE_STEP_MS;
            (1.0, inner.y + inner.height.saturating_sub(swept), seed)
        }
        EasterEggPhase::Idle | EasterEggPhase::Active => return,
    };

    let buf = frame.buffer_mut();

    for row in 0..inner.height {
        let y = inner.y + row;
        // 已被扫过的行留给下层的彩蛋内容。
        if y >= sweep_line {
            continue;
        }
        for col in 0..inner.width {
            let x = inner.x + col;
            if !mascot::cell_filled(col, row, progress) {
                continue;
            }
            let (upper, lower) = mascot::noise_cell_colors(col, row, seed);
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(mascot::HALF_BLOCK)
                    .set_style(Style::default().fg(upper).bg(lower));
            }
        }
    }
}

/// 彩蛋激活后的边框：噪点色沿边框逆时针循环流动。
///
/// 覆盖整圈边框，底部那一行正是版本文字所在处，因此文字也随之流动。
#[cfg(feature = "easter-egg")]
fn draw_about_border_flow(frame: &mut Frame, app: &App, area: Rect) {
    use crate::render::mascot;

    if area.width < 2 || area.height < 2 {
        return;
    }
    let Some(started_at) = app.about_egg.mascot_activated_at else {
        return;
    };

    // 相位随时间推进，颜色便沿环行进；除数越小流动越快。
    let phase = started_at.elapsed().as_millis() as u64 / 45;

    let left = area.x;
    let right = area.x + area.width - 1;
    let top = area.y;
    let bottom = area.y + area.height - 1;

    // 逆时针：左上 → 下（左边）→ 右（底边）→ 上（右边）→ 左（顶边）。
    let mut ring: Vec<(u16, u16)> =
        Vec::with_capacity((area.width as usize + area.height as usize) * 2 - 4);
    for y in top..=bottom {
        ring.push((left, y));
    }
    for x in (left + 1)..=right {
        ring.push((x, bottom));
    }
    for y in (top..bottom).rev() {
        ring.push((right, y));
    }
    for x in ((left + 1)..right).rev() {
        ring.push((x, top));
    }

    let buf = frame.buffer_mut();
    for (pos, (x, y)) in ring.into_iter().enumerate() {
        let color = mascot::border_flow_color(pos as u16, phase);
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_fg(color);
        }
    }
}

/// 彩蛋激活后的形象：在内容区居中，点击可播放果冻动画。
#[cfg(feature = "easter-egg")]
fn draw_about_mascot(frame: &mut Frame, app: &mut App, inner: Rect) {
    use crate::render::{mascot, mascot_frames};

    let frame_data = match app.about_egg.jelly_started_at {
        Some(started_at) => mascot::jelly_frame(started_at.elapsed()),
        None => mascot::idle_frame(),
    };

    // 命中区域固定用基准尺寸：果冻形变时形象忽胖忽瘦，跟着变会让点击手感发飘。
    let hit_rect = centered_in(inner, mascot_frames::BASE_WIDTH, mascot_frames::BASE_HEIGHT);
    app.about_egg.mascot_hit = Some(crate::app::HitRect {
        x: hit_rect.x,
        y: hit_rect.y,
        width: hit_rect.width,
        height: hit_rect.height,
    });

    let draw_rect = centered_in(inner, frame_data.width, frame_data.height);
    if draw_rect.width == 0 || draw_rect.height == 0 {
        return;
    }

    frame.render_widget(
        Paragraph::new(mascot::frame_lines(frame_data, app.theme.color_surface()))
            .style(Style::default().bg(app.theme.color_surface())),
        draw_rect,
    );
}

/// 在 `outer` 内居中放置一个 `width` x `height` 的区域，并裁到 `outer` 之内。
#[cfg(feature = "easter-egg")]
fn centered_in(outer: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(outer.width);
    let h = height.min(outer.height);
    Rect {
        x: outer.x + outer.width.saturating_sub(w) / 2,
        y: outer.y + outer.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

fn draw_about_braille(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let lines = about_braille_lines(area.width as usize, area.height as usize);
    let p = Paragraph::new(lines).style(
        Style::default()
            .fg(app.theme.color_text())
            .bg(app.theme.color_surface()),
    );
    frame.render_widget(p, area);
}

fn draw_about_text(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let info = about_info();
    let max_width = area.width as usize;
    if max_width == 0 {
        return;
    }

    let mut rendered: Vec<String> = Vec::new();
    for (k, v) in &info.links {
        let line = if k.eq_ignore_ascii_case("github_url") {
            v.to_string()
        } else {
            format!("{}: {}", k, v)
        };
        rendered.extend(wrap_text(&line, max_width));
    }

    let desc_lines: Vec<String> = wrap_text(&info.description, max_width);
    if !desc_lines.is_empty() {
        rendered.push(String::new());
        rendered.extend(desc_lines);
    }

    let max_line_width = rendered
        .iter()
        .map(|l| unicode_width::UnicodeWidthStr::width(l.as_str()))
        .max()
        .unwrap_or(0)
        .min(max_width);
    let block_h = rendered.len() as u16;
    let block_w = max_line_width.max(1) as u16;
    let offset_x = (area.width.saturating_sub(block_w)) / 2;
    let offset_y = if block_h <= area.height {
        (area.height.saturating_sub(block_h)) / 2
    } else {
        0
    };

    let lines: Vec<Line> = rendered
        .into_iter()
        .map(|l| {
            Line::styled(
                l,
                Style::default()
                    .fg(app.theme.color_text())
                    .bg(app.theme.color_surface()),
            )
        })
        .collect();
    let p = Paragraph::new(lines)
        .style(Style::default().bg(app.theme.color_surface()))
        .wrap(ratatui::widgets::Wrap { trim: false });
    let text_h = area.height.saturating_sub(offset_y).min(block_h.max(1));
    let text_area = Rect {
        x: area.x + offset_x,
        y: area.y + offset_y,
        width: block_w,
        height: text_h,
    };
    frame.render_widget(p, text_area);
}

fn wrap_text(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    if s.is_empty() {
        return vec![String::new()];
    }

    // Wrap on display columns, not char count: CJK text is twice as wide as it
    // is long, and counting chars would clip it.
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut buf_width = 0usize;
    for ch in s.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if buf_width + ch_width > width && !buf.is_empty() {
            out.push(std::mem::take(&mut buf));
            buf_width = 0;
        }
        buf.push(ch);
        buf_width += ch_width;
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

fn about_braille_lines(width: usize, height: usize) -> Vec<Line<'static>> {
    let blank = " ".repeat(width);
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let info = about_info();
    let Some(selected) = select_about_braille_art(width, height, &info.braille_images) else {
        return (0..height).map(|_| Line::from(blank.clone())).collect();
    };

    let mut rows: Vec<String> = selected
        .art
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect();

    let mut start = 0usize;
    let mut end = rows.len();
    while start < end && rows[start].trim().is_empty() {
        start += 1;
    }
    while end > start && rows[end - 1].trim().is_empty() {
        end -= 1;
    }
    rows = rows[start..end].to_vec();

    let rows_w = rows
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let canvas_w = selected.width.max(rows_w);
    let canvas_h = selected.height.max(rows.len());

    let offset_x = width.saturating_sub(canvas_w) / 2;
    let offset_y = height.saturating_sub(canvas_h) / 2;
    let mut grid: Vec<Vec<char>> = vec![vec![' '; width]; height];

    for (row_idx, row) in rows.iter().enumerate() {
        let gy = offset_y + row_idx;
        if gy >= height {
            break;
        }
        for (col_idx, ch) in row.chars().enumerate() {
            let gx = offset_x + col_idx;
            if gx >= width {
                break;
            }
            grid[gy][gx] = ch;
        }
    }

    grid.into_iter()
        .map(|row| Line::from(row.into_iter().collect::<String>()))
        .collect()
}

fn select_about_braille_art(
    width: usize,
    height: usize,
    arts: &[BrailleImage],
) -> Option<&BrailleImage> {
    let mut best_fit: Option<(&BrailleImage, u128)> = None;
    for art in arts {
        if art.width == 0 || art.height == 0 {
            continue;
        }
        if art.width <= width && art.height <= height {
            let score = (art.width as u128) * (art.height as u128);
            let should_replace = best_fit
                .as_ref()
                .map(|(_, best_score)| score > *best_score)
                .unwrap_or(true);
            if should_replace {
                best_fit = Some((art, score));
            }
        }
    }

    if let Some((art, _)) = best_fit {
        return Some(art);
    }

    arts.iter()
        .filter(|art| art.width > 0 && art.height > 0)
        .min_by_key(|art| {
            let dw = art.width.saturating_sub(width) as u128;
            let dh = art.height.saturating_sub(height) as u128;
            let overflow = dw.saturating_mul(dh).saturating_add(dw).saturating_add(dh);
            let area = (art.width as u128).saturating_mul(art.height as u128);
            (overflow, area)
        })
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width.saturating_sub(2)).max(12);
    let h = height.min(area.height.saturating_sub(2)).max(5);
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

fn base_bg_style(app: &App) -> Style {
    Style::default()
        .fg(app.theme.color_subtext())
        .bg(app.theme.color_surface())
}

fn audio_quality_label(app: &App, quality: AudioQuality) -> &'static str {
    match app.config.language {
        Language::Zh => match quality {
            AudioQuality::Standard => "标准",
            AudioQuality::Higher => "较高",
            AudioQuality::Exhigh => "极高",
            AudioQuality::Lossless => "无损",
            AudioQuality::Hires => "Hi-Res",
            AudioQuality::Jyeffect => "高清环绕声",
            AudioQuality::Sky => "沉浸环绕声",
            AudioQuality::Dolby => "杜比全景声",
            AudioQuality::Jymaster => "超清母带",
        },
        Language::En => match quality {
            AudioQuality::Standard => "Standard",
            AudioQuality::Higher => "Higher",
            AudioQuality::Exhigh => "Exhigh",
            AudioQuality::Lossless => "Lossless",
            AudioQuality::Hires => "Hi-Res",
            AudioQuality::Jyeffect => "JYEffect",
            AudioQuality::Sky => "Sky",
            AudioQuality::Dolby => "Dolby",
            AudioQuality::Jymaster => "JYMaster",
        },
    }
}

fn l<'a>(app: &App, zh: &'a str, en: &'a str) -> &'a str {
    match app.config.language {
        Language::Zh => zh,
        Language::En => en,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 编辑态横向窗口：光标在窗口内不动；撞到边界时**一次只挪一个字符**。
    #[test]
    fn path_window_only_scrolls_at_the_edges() {
        // 窗口内：不动（文本不跟着每一次光标移动一起滑）。
        assert_eq!(adjust_path_window(10, 15, 20), 10);
        assert_eq!(adjust_path_window(0, 19, 20), 0);

        // 撞右边界：只挪一格，光标贴回右端。
        assert_eq!(adjust_path_window(0, 20, 20), 1);
        assert_eq!(adjust_path_window(5, 25, 20), 6);
        assert!(agent_in_window(20, adjust_path_window(0, 20, 20), 20));

        // 撞左边界：只挪一格。
        assert_eq!(adjust_path_window(10, 9, 20), 9);
        assert!(agent_in_window(9, adjust_path_window(10, 9, 20), 20));

        // 一次性跳很远（Home / End / 点击）：对齐到光标那一端。
        assert_eq!(adjust_path_window(100, 5, 20), 5);
        assert_eq!(adjust_path_window(0, 200, 20), 181);
        assert!(agent_in_window(200, adjust_path_window(0, 200, 20), 20));

        // 宽度 0 时不滚动。
        assert_eq!(adjust_path_window(7, 0, 0), 0);
    }

    fn agent_in_window(caret_col: usize, window: usize, width: usize) -> bool {
        caret_col >= window && caret_col < window + width
    }

    #[test]
    fn caret_column_counts_wide_chars() {
        // "/tmp/" 5 列；再接 "音乐" 各占两列。
        assert_eq!(caret_display_col("/tmp/音乐", 5), 5);
        assert_eq!(caret_display_col("/tmp/音乐", 6), 7);
        assert_eq!(caret_display_col("/tmp/音乐", 7), 9);
        assert_eq!(caret_display_col("/tmp/音乐", 0), 0);
    }

    /// 窗口串：从 `start_col` 起截断，光标下标按窗口内字符位置给出。
    #[test]
    fn path_window_slices_from_the_start_column() {
        let (visible, caret) = path_window("abcdef", 6, 3, 2);
        assert_eq!(visible, "cde");
        assert_eq!(caret, visible.chars().count(), "光标在串尾时跟在最后");

        let (visible, caret) = path_window("abcdef", 3, 3, 2);
        assert_eq!(visible, "cde");
        assert_eq!(caret, 1);
    }

    /// 焦点条目必须始终落在可视窗口内——否则键盘选中的那一行点不到，
    /// 鼠标点的行也不是看到的那一行。
    #[test]
    fn focused_keybind_row_is_always_inside_the_window() {
        let total = crate::app::SETTINGS_KEYBIND_ITEMS + 3;

        for visible_rows in 1..=total {
            for focus in 0..crate::app::SETTINGS_KEYBIND_ITEMS {
                let scroll = scroll_for_focus(total, visible_rows, focus);
                let end = (scroll + visible_rows).min(total);

                assert!(
                    scroll <= focus && focus < end,
                    "visible_rows={visible_rows} focus={focus} scroll={scroll}"
                );
            }
        }
    }

    #[test]
    fn scroll_is_zero_when_everything_fits() {
        assert_eq!(scroll_for_focus(23, 23, 19), 0);
        assert_eq!(scroll_for_focus(23, 40, 19), 0);
    }

    /// 命中区行号必须与渲染行号一致：条目 i 画在 `i - scroll` 行。
    #[test]
    fn scrolled_rows_match_the_rendered_offsets() {
        for scroll in [0usize, 1, 7, 12] {
            let rows = scrolled_rows(crate::app::SETTINGS_KEYBIND_ITEMS, 8, scroll);
            for (idx, offset) in &rows {
                assert_eq!(*idx as isize - *offset as isize, scroll as isize);
                assert!(*offset < 8);
            }
            // 窗口内的条目一个不漏
            let expected: Vec<usize> = (scroll..crate::app::SETTINGS_KEYBIND_ITEMS)
                .take(8)
                .collect();
            let got: Vec<usize> = rows.iter().map(|(idx, _)| *idx).collect();
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn scrolled_rows_is_empty_without_room() {
        assert!(scrolled_rows(crate::app::SETTINGS_KEYBIND_ITEMS, 0, 3).is_empty());
    }
}

fn on_off(app: &App, enabled: bool) -> &'static str {
    match app.config.language {
        Language::Zh => {
            if enabled {
                "开"
            } else {
                "关"
            }
        }
        Language::En => {
            if enabled {
                "On"
            } else {
                "Off"
            }
        }
    }
}
