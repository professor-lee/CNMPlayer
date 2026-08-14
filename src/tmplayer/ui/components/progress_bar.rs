use crate::tmplayer::app::state::AppState;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// 脉冲动画的时间基准：进程启动时初始化。
/// 注意不能用 Instant::now().elapsed()——那是“当前时刻到当前时刻”，恒为 0，
/// 会导致波形静止不动。
static ANIM_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

pub fn render(f: &mut Frame, area: Rect, app: &AppState, pos: Duration, dur: Duration) {
    let w = area.width as usize;
    if w == 0 {
        return;
    }

    let ratio = if dur.as_secs_f32() > 0.0 {
        (pos.as_secs_f32() / dur.as_secs_f32()).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // knob moves on [0, w-1]
    let knob = if w <= 1 {
        0usize
    } else {
        (ratio * (w as f32 - 1.0)).round() as usize
    };

    let left = "─".repeat(knob);
    let right = if w > 0 {
        "─".repeat(w.saturating_sub(1 + knob))
    } else {
        String::new()
    };

    let line = if app.player.seeking {
        // 宿主正在后台加载跳转目标：已播放区域（横线部分）显示从左向右的
        // 脉冲波，颜色 = 当前进度条颜色（主题色 accent2）稍作提亮，随波动态
        // 变化，基础颜色来自主题，不硬编码颜色。
        let phase = ANIM_EPOCH.elapsed().as_secs_f32() * 7.0;
        let mut spans = Vec::with_capacity(knob + 2);
        for x in 0..knob {
            let wave = ((phase - x as f32 * 0.3).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
            let color = app.theme.lighten(app.theme.palette.accent2, 0.25 * wave);
            spans.push(Span::styled("─", Style::default().fg(color)));
        }
        spans.push(Span::styled(
            "○",
            Style::default().fg(app.theme.color_accent()),
        ));
        spans.push(Span::styled(
            right,
            Style::default().fg(app.theme.color_subtext()),
        ));
        Line::from(spans)
    } else {
        Line::from(vec![
            Span::styled(left, Style::default().fg(app.theme.color_accent2())),
            Span::styled("○", Style::default().fg(app.theme.color_accent())),
            Span::styled(right, Style::default().fg(app.theme.color_subtext())),
        ])
    };

    f.render_widget(Paragraph::new(line), area);
}
