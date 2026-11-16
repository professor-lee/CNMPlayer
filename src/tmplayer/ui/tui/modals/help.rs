use super::super::layout::{ModalRows, centered_rect};
use super::super::text::lang_text;
use crate::data::keymap::{self, KeyRow, KeyView};
use crate::tmplayer::app::state::AppState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

/// 按键提示弹窗的行：只列全屏页里生效的键（含分组标题），与主程序按键绑定页同源。
pub fn help_rows() -> &'static [KeyRow] {
    keymap::key_rows(KeyView::Fullscreen)
}

/// 一行的展示文本（标题为分组名，其余为 `名称: 按键`）。
pub fn help_row_text(app: &AppState, row: KeyRow) -> String {
    keymap::row_text(row, &app.config, app.language, KeyView::Fullscreen)
}

pub(in crate::tmplayer::ui::tui) fn render_help_modal(
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
        .title(lang_text(app, " 按键绑定 ", " Keybinds "))
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

    let items = help_rows();

    let visible_rows = rows[1].height as usize;
    let total_rows = items.len();
    let selected = app.help_keybind_selected.min(total_rows.saturating_sub(1));
    // 与主应用按键绑定页共用的越界聚焦滚动：焦点在窗口内视口不动，上滚时带出分组标题。
    let scroll = keymap::scroll_for_row(app.help_keybind_scroll, items, visible_rows, selected);
    app.help_keybind_scroll = scroll;

    for (idx, &row) in items.iter().enumerate().skip(scroll).take(visible_rows) {
        let rect = Rect {
            x: rows[1].x,
            y: rows[1].y + (idx - scroll) as u16,
            width: rows[1].width,
            height: 1,
        };
        let text = help_row_text(app, row);
        let line = match row {
            KeyRow::Header(_) => Line::styled(
                text,
                Style::default()
                    .fg(app.theme.color_accent3())
                    .add_modifier(Modifier::BOLD),
            ),
            _ => {
                let style = if idx == selected {
                    Style::default()
                        .fg(app.theme.color_accent2())
                        .add_modifier(Modifier::BOLD)
                } else if matches!(row, KeyRow::Fixed(_)) {
                    Style::default().fg(app.theme.color_subtext())
                } else {
                    Style::default().fg(app.theme.color_text())
                };
                // 标题行不可聚焦，也不登记命中区。
                modal_rows.push(rect, idx);
                Line::styled(format!("  {text}"), style)
            }
        };
        f.render_widget(Paragraph::new(line), rect);
    }

    f.render_widget(
        Paragraph::new(lang_text(
            app,
            "Up/Down 浏览  Ctrl+K 关闭  Esc 返回（仅查看）",
            "Up/Down browse  Ctrl+K close  Esc back (view only)",
        ))
        .style(Style::default().fg(app.theme.color_subtext())),
        rows[2],
    );
}
