use super::{MprisName, resolve_mpris_name};

#[test]
fn mpris_name_templates_preserve_text_and_replace_only_the_supported_token() {
    for (template, title, expected) in [
        ("", None, "CNMPlayer"),
        (" \t ", Some("歌曲"), "CNMPlayer"),
        ("我的播放器", Some("歌曲"), "我的播放器"),
        ("CNMPlayer · @play", Some("晴天"), "CNMPlayer · 晴天"),
        ("@play / @play", Some("歌名"), "歌名 / 歌名"),
        ("@pause @play", Some("@play"), "@pause @play"),
        (" @play ", None, " CNMPlayer "),
        ("@play", Some("   "), "CNMPlayer"),
    ] {
        assert_eq!(resolve_mpris_name(template, title), expected);
    }
}

#[test]
fn mpris_name_follows_track_and_configuration_changes_without_duplicate_updates() {
    let mut name = MprisName::new("正在播放：@play", None);
    assert_eq!(&*name.identity, "正在播放：CNMPlayer");
    assert!(name.update("正在播放：@play", Some("第一首")));
    assert_eq!(&*name.identity, "正在播放：第一首");
    assert!(!name.update("正在播放：@play", Some("第一首")));
    assert!(name.update("正在播放：@play", Some("第二首")));
    assert_eq!(&*name.identity, "正在播放：第二首");
    assert!(name.update("我的播放器", Some("第二首")));
    assert_eq!(&*name.identity, "我的播放器");
    assert!(!name.update("我的播放器", Some("第三首")));
    assert!(name.update("", Some("第三首")));
    assert_eq!(&*name.identity, "CNMPlayer");
    assert!(!name.update("CNMPlayer", None));
    assert!(name.update("@play", Some("第四首")));
    assert!(name.update("@play", None));
    assert_eq!(&*name.identity, "CNMPlayer");
}
