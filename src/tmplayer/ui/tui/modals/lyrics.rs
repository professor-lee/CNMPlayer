use super::super::layout::{ModalRows, centered_rect};
use super::super::text::{lang_on_off, lang_text};
use crate::tmplayer::app::state::AppState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

pub(in crate::tmplayer::ui::tui) fn render_lyrics_settings_modal(
    f: &mut ratatui::Frame,
    size: Rect,
    app: &mut AppState,
    modal_rows: &mut ModalRows,
) {
    let area = centered_rect(size, 70, 20);
    f.render_widget(ratatui::widgets::Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(crate::tmplayer::ui::borders::SOLID_BORDER)
        .title(lang_text(app, " 歌词 ", " Lyrics "))
        .style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        );
    f.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    f.render_widget(Paragraph::new(""), rows[0]);

    // 与主应用设置同构：吸附行只在拖动开启时可改，关闭时灰置。
    let drag_enabled = app.config.page_lyrics_drag;
    let items = [
        format!(
            "{}: {}",
            lang_text(app, "歌词浮窗", "Lyrics Overlay"),
            lang_on_off(app, app.config.page_lyrics)
        ),
        format!(
            "{}: {}",
            lang_text(app, "歌词浮窗拖动", "Lyrics Overlay Drag"),
            lang_on_off(app, drag_enabled)
        ),
        format!(
            "{}: {}",
            lang_text(app, "歌词浮窗边缘吸附", "Lyrics Overlay Edge Snap"),
            lang_on_off(app, app.config.page_lyrics_snap)
        ),
        format!(
            "{}: {}",
            lang_text(app, "逐字歌词", "Word-by-word Lyrics"),
            lang_on_off(app, app.config.word_lyrics)
        ),
        format!(
            "{}: {}",
            lang_text(app, "歌词翻译", "Lyrics Translation"),
            lang_on_off(app, app.config.lyric_translation)
        ),
    ];

    for (idx, text) in items.iter().enumerate() {
        if idx as u16 >= rows[1].height {
            break;
        }
        let disabled = idx == 2 && !drag_enabled;
        let style = if idx == app.lyrics_settings_selected {
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
        let rect = Rect {
            x: rows[1].x,
            y: rows[1].y + idx as u16,
            width: rows[1].width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::styled(format!("  {}", text), style)),
            rect,
        );
        modal_rows.push(rect, idx);
    }

    f.render_widget(Paragraph::new(""), rows[2]);
}
