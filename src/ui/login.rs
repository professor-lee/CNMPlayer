use crate::app::App;
use crate::app::auth::LoginMethod;
use crate::data::config::Language;
use qrcode::QrCode;
use qrcode::render::unicode;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

const DEFAULT_OPENING_TITLE: &str = " ██████╗███╗   ██╗███╗   ███╗██████╗ ██╗      █████╗ ██╗   ██╗███████╗██████╗ \n██╔════╝████╗  ██║████╗ ████║██╔══██╗██║     ██╔══██╗╚██╗ ██╔╝██╔════╝██╔══██╗\n██║     ██╔██╗ ██║██╔████╔██║██████╔╝██║     ███████║ ╚████╔╝ █████╗  ██████╔╝\n██║     ██║╚██╗██║██║╚██╔╝██║██╔═══╝ ██║     ██╔══██║  ╚██╔╝  ██╔══╝  ██╔══██╗\n╚██████╗██║ ╚████║██║ ╚═╝ ██║██║     ███████╗██║  ██║   ██║   ███████╗██║  ██║\n ╚═════╝╚═╝  ╚═══╝╚═╝     ╚═╝╚═╝     ╚══════╝╚═╝  ╚═╝   ╚═╝   ╚══════╝╚═╝  ╚═╝";

pub fn draw_login(frame: &mut Frame, app: &App) {
    let size = frame.area();

    if size.width < 36 || size.height < 12 {
        draw_too_small(frame, app, size);
        return;
    }

    frame.render_widget(Block::default().style(base_bg_style(app)), size);

    let title_height = (size.height / 3).max(3);
    let hint_height = 1;
    let form_height_zone = size.height.saturating_sub(title_height + hint_height);

    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(title_height),
            Constraint::Length(form_height_zone),
            Constraint::Length(hint_height),
        ])
        .split(size);

    let title_block = opening_title_block(&app.config.default_opening_title);
    let title_lines = title_block.lines().count().max(1) as u16;
    let title_area = centered_rect(areas[0].width, title_lines.min(areas[0].height), areas[0]);
    frame.render_widget(
        Paragraph::new(title_block)
            .style(
                Style::default()
                    .fg(app.theme.color_accent())
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center),
        title_area,
    );

    let login_content = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(areas[1]);

    let form_height = form_height(app.login.method).min(login_content[0].height);
    let form_width = login_content[0].width.saturating_sub(2).clamp(28, 72);
    let inner = centered_rect(form_width, form_height, login_content[0]);

    let content = Paragraph::new(build_form_lines(app))
        .style(Style::default().fg(app.theme.color_text()))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true });
    frame.render_widget(content, inner);

    render_qr(frame, app, login_content[1]);

    let left_hint = "F1/F2/F3 切换登录方式  Tab/↑↓ 切换焦点  Enter 确认";
    frame.render_widget(
        Paragraph::new(lang_text(
            app,
            left_hint,
            "F1/F2/F3 switch login mode  Tab/Up/Down switch focus  Enter confirm",
        ))
        .style(Style::default().fg(app.theme.color_subtext()))
        .alignment(Alignment::Left),
        areas[2],
    );
}

fn draw_too_small(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(Block::default().style(base_bg_style(app)), area);
    let msg = Paragraph::new(lang_text(app, "终端窗口过小", "Terminal too small"))
        .style(Style::default().fg(app.theme.color_subtext()))
        .alignment(Alignment::Center);
    frame.render_widget(msg, area);
}

fn opening_title_block(custom: &str) -> String {
    if custom.trim().is_empty() {
        return DEFAULT_OPENING_TITLE.to_string();
    }

    // Allow literal "\\n" in config to become line breaks.
    custom.replace("\\n", "\n")
}

fn form_height(method: LoginMethod) -> u16 {
    match method {
        LoginMethod::Qr => 8,
        LoginMethod::Username => 9,
        LoginMethod::Phone => 10,
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn build_form_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    match app.login.method {
        LoginMethod::Qr => {
            push_action_line(
                &mut lines,
                app,
                0,
                format!("󰐑 {}", lang_text(app, "刷新二维码", "Refresh QR")),
                app.login.focus_index == 0,
            );
            push_action_line(
                &mut lines,
                app,
                1,
                format!(
                    "󰦏 {}",
                    lang_text(app, "已扫码，确认登录", "Scanned, Confirm Login")
                ),
                app.login.focus_index == 1,
            );
            if !app.login.qr_url.trim().is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("{}: {}", lang_text(app, "二维码", "QR"), app.login.qr_url),
                    Style::default().fg(app.theme.color_subtext()),
                )));
            }
        }
        LoginMethod::Username => {
            push_input_line(
                &mut lines,
                app,
                0,
                &format!("󰀄 {}", lang_text(app, "用户名", "Username")),
                &app.login.username,
                false,
                app.login.focus_index == 0,
            );
            push_input_line(
                &mut lines,
                app,
                1,
                &format!("󰍛 {}", lang_text(app, "密码", "Password")),
                &app.login.password,
                true,
                app.login.focus_index == 1,
            );
            push_action_line(
                &mut lines,
                app,
                2,
                format!("󰌋 {}", lang_text(app, "登录", "Login")),
                app.login.focus_index == 2,
            );
        }
        LoginMethod::Phone => {
            push_input_line(
                &mut lines,
                app,
                0,
                &format!("󰌘 {}", lang_text(app, "手机号", "Phone")),
                &app.login.phone,
                false,
                app.login.focus_index == 0,
            );
            push_input_line(
                &mut lines,
                app,
                1,
                &format!("󰣖 {}", lang_text(app, "验证码", "Captcha")),
                &app.login.captcha,
                false,
                app.login.focus_index == 1,
            );
            push_action_line(
                &mut lines,
                app,
                2,
                format!("\u{f1d8} {}", lang_text(app, "发送验证码", "Send Captcha")),
                app.login.focus_index == 2,
            );
            push_action_line(
                &mut lines,
                app,
                3,
                format!("󰌋 {}", lang_text(app, "登录", "Login")),
                app.login.focus_index == 3,
            );
        }
    }

    lines
}

fn render_qr(frame: &mut Frame, app: &App, area: Rect) {
    if app.login.method != LoginMethod::Qr || area.width < 10 || area.height < 6 {
        return;
    }

    let payload = if !app.login.qr_url.trim().is_empty() {
        app.login.qr_url.trim()
    } else {
        app.login.qr_key.trim()
    };

    if payload.is_empty() {
        return;
    }

    let Ok(code) = QrCode::new(payload.as_bytes()) else {
        return;
    };

    let image = code
        .render::<unicode::Dense1x2>()
        .quiet_zone(false)
        .dark_color(unicode::Dense1x2::Dark)
        .light_color(unicode::Dense1x2::Light)
        .build();

    frame.render_widget(
        Paragraph::new(image)
            .style(Style::default().fg(app.theme.color_text()))
            .alignment(Alignment::Center),
        area,
    );
}

fn push_action_line(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    _index: usize,
    label: String,
    focused: bool,
) {
    let style = if focused {
        Style::default()
            .fg(app.theme.color_text())
            .bg(app.theme.color_surface())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.color_text())
    };

    lines.push(Line::from(Span::styled(label, style)));
}

fn push_input_line(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    _index: usize,
    label: &str,
    value: &str,
    is_password: bool,
    focused: bool,
) {
    let shown = if is_password {
        "•".repeat(value.chars().count())
    } else {
        value.to_string()
    };

    let cursor = if focused { "▌" } else { "" };
    let row_style = if focused {
        Style::default().bg(app.theme.color_surface())
    } else {
        Style::default()
    };
    let value_style = if value.is_empty() {
        Style::default().fg(app.theme.color_subtext())
    } else {
        Style::default().fg(app.theme.color_text())
    };

    lines.push(Line::from(vec![
        Span::styled(
            format!("{}: ", label),
            row_style.fg(app.theme.color_accent2()),
        ),
        Span::styled(format!("{}{}", shown, cursor), row_style.patch(value_style)),
    ]));
}

fn base_bg_style(app: &App) -> Style {
    if app.config.transparent_background {
        Style::default()
    } else {
        Style::default().bg(app.theme.color_base())
    }
}

fn lang_text<'a>(app: &App, zh: &'a str, en: &'a str) -> &'a str {
    match app.config.language {
        Language::Zh => zh,
        Language::En => en,
    }
}
