use super::super::layout::{ModalRows, centered_rect};
use super::super::text::{lang_on_off, lang_text};
use crate::tmplayer::app::state::AppState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

pub(in crate::tmplayer::ui::tui) fn render_bar_settings_modal(
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
        .title(lang_text(app, " 播放设置 ", " Playback Settings "))
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

    let bar_number_label = match app.config.bar_number {
        crate::data::config::BarNumber::Auto => lang_text(app, "自动", "Auto"),
        crate::data::config::BarNumber::N16 => "16",
        crate::data::config::BarNumber::N32 => "32",
        crate::data::config::BarNumber::N48 => "48",
        crate::data::config::BarNumber::N64 => "64",
        crate::data::config::BarNumber::N80 => "80",
        crate::data::config::BarNumber::N96 => "96",
    };
    let channels_label = match app.config.bar_channels {
        crate::data::config::BarChannels::Mono => "Mono",
        crate::data::config::BarChannels::Stereo => "Stereo",
    };

    let items = [
        format!(
            "{}: {}",
            lang_text(app, "可视化", "Visualization"),
            match app.config.visualize {
                crate::data::config::VisualizeMode::Lyrics => lang_text(app, "仅歌词", "Lyrics"),
                crate::data::config::VisualizeMode::Hidden => lang_text(app, "关闭", "Off"),
                crate::data::config::VisualizeMode::Bars => lang_text(app, "频谱", "Bars"),
                crate::data::config::VisualizeMode::Oscilloscope => {
                    lang_text(app, "示波器", "Oscilloscope")
                }
                crate::data::config::VisualizeMode::Vector => {
                    lang_text(app, "矢量", "Vector")
                }
            }
        ),
        format!(
            "{}: {}",
            lang_text(app, "频谱间隔", "Bars Gap"),
            lang_on_off(app, app.config.bars_gap)
        ),
        format!(
            "{}: {}",
            lang_text(app, "频谱数", "Bars Count"),
            bar_number_label
        ),
        format!("{}: {}", lang_text(app, "声道", "Channels"), channels_label),
        format!(
            "{}: {}",
            lang_text(app, "封面边框", "Cover Border"),
            lang_on_off(app, app.config.album_border)
        ),
        format!(
            "{}: {}",
            lang_text(app, "音质", "Audio Quality"),
            match app.config.audio_quality {
                crate::data::config::AudioQuality::Standard => lang_text(app, "标准", "Standard"),
                crate::data::config::AudioQuality::Higher => lang_text(app, "较高", "Higher"),
                crate::data::config::AudioQuality::Exhigh => lang_text(app, "极高", "Exhigh"),
                crate::data::config::AudioQuality::Lossless => lang_text(app, "无损", "Lossless"),
                crate::data::config::AudioQuality::Hires => "Hi-Res",
                crate::data::config::AudioQuality::Jyeffect => {
                    lang_text(app, "高清环绕声", "JYEffect")
                }
                crate::data::config::AudioQuality::Sky => {
                    lang_text(app, "沉浸环绕声", "Sky")
                }
                crate::data::config::AudioQuality::Dolby => {
                    lang_text(app, "杜比全景声", "Dolby")
                }
                crate::data::config::AudioQuality::Jymaster => {
                    lang_text(app, "超清母带", "JYMaster")
                }
            }
        ),
        format!(
            "{}: {}",
            lang_text(app, "播放记忆", "Playback Memory"),
            lang_on_off(app, app.config.playback_memory)
        ),
    ];

    for (idx, text) in items.iter().enumerate() {
        if idx as u16 >= rows[1].height {
            break;
        }
        let style = if idx == app.bar_settings_selected {
            Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD)
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
