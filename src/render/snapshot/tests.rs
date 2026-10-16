use super::*;
use ratatui::{backend::TestBackend, widgets::Paragraph};

#[test]
fn preparing_a_new_host_snapshot_does_not_replace_the_visible_fullscreen_frame() {
    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
    terminal
        .draw(|frame| {
            frame.render_widget(Paragraph::new("FULLSCREEN 播放中"), frame.area());
        })
        .unwrap();
    let visible = terminal.backend().buffer().clone();
    let snapshot = capture(&mut terminal, |frame| {
        frame.render_widget(Paragraph::new("HOST 暂停后返回"), frame.area());
    })
    .unwrap();
    assert_eq!(terminal.backend().buffer(), &visible);
    assert_eq!(snapshot[(0, 0)].symbol(), "H");
    assert_eq!(snapshot[(5, 0)].symbol(), "暂");
    terminal
        .draw(|frame| {
            frame
                .buffer_mut()
                .content
                .clone_from_slice(&snapshot.content);
        })
        .unwrap();
    assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "H");
    assert_eq!(terminal.backend().buffer()[(5, 0)].symbol(), "暂");
}
