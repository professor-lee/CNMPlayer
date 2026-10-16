use crate::tmplayer::app::state::AppState;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;

pub fn render(f: &mut Frame, area: Rect, app: &AppState, vol: f32) {
    if area.is_empty() {
        return;
    }

    const LEVELS: [&str; 9] = [" ", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];
    let inner = area.width.saturating_sub(2);
    let filled = (vol.clamp(0.0, 1.0) * f32::from(inner) * 8.0).round() as u32;
    let accent = Style::default().fg(app.theme.color_accent());
    let subtext = Style::default().fg(app.theme.color_subtext());
    let buffer = f.buffer_mut();

    if let Some(cell) = buffer.cell_mut((area.x, area.y)) {
        cell.set_symbol("[").set_style(subtext);
    }
    if area.width == 1 {
        return;
    }
    for index in 0..inner {
        let level = filled.saturating_sub(u32::from(index) * 8).min(8) as usize;
        if let Some(cell) = buffer.cell_mut((area.x + index + 1, area.y)) {
            cell.set_symbol(LEVELS[level])
                .set_style(if level > 0 { accent } else { subtext });
        }
    }
    if let Some(cell) = buffer.cell_mut((area.right() - 1, area.y)) {
        cell.set_symbol("]").set_style(subtext);
    }
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::data::config::{Config, Language};
    use crate::tmplayer::app::state::AppState;
    use crate::ui::theme::Theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    fn rendered(width: u16, volume: f32) -> String {
        let app = AppState::new(Config::default(), Theme::default(), Language::En);
        let mut terminal = Terminal::new(TestBackend::new(width.max(1), 1)).unwrap();
        terminal
            .draw(|frame| render(frame, Rect::new(0, 0, width, 1), &app, volume))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn volume_renders_each_eighth_cell() {
        for (step, expected) in [
            "[ ]", "[▏]", "[▎]", "[▍]", "[▌]", "[▋]", "[▊]", "[▉]", "[█]",
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(rendered(3, step as f32 / 8.0), expected);
        }
        assert_eq!(rendered(6, 0.34375), "[█▍  ]");
        assert_eq!(rendered(6, -0.5), "[    ]");
        assert_eq!(rendered(6, 1.5), "[████]");
    }

    #[test]
    fn volume_handles_a_bar_too_narrow_to_fill() {
        assert_eq!(rendered(0, 1.0), " ");
        assert_eq!(rendered(1, 1.0), "[");
        assert_eq!(rendered(2, 1.0), "[]");
    }
}
