use super::App;
use super::Page;
use super::api::ApiState;
use super::response::parse_value_as_string;
use super::response::response_code;
use super::response::response_message;
use super::roam::fetch_private_roam_songs;
use super::search_controller::SearchController;
use super::sidebar::HomeSidebarState;
use super::text::MAX_INPUT_LEN;
use super::text::lang_text;
use super::text::truncate_text;
use crate::data::config::AudioQuality;
use crate::data::config::Language;
use crate::data::persistence::PersistenceKey;
use crate::data::private_roam;
use crate::data::session;
use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ncm_api::ApiResponse;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginMethod {
    Qr,
    Username,
    Phone,
}

pub(crate) struct LoginState {
    pub(crate) method: LoginMethod,
    pub(crate) focus_index: usize,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) phone: String,
    pub(crate) captcha: String,
    pub(crate) qr_key: String,
    pub(crate) qr_url: String,
    pub(crate) status_line: String,
}

impl Default for LoginState {
    fn default() -> Self {
        Self {
            method: LoginMethod::Qr,
            focus_index: 0,
            username: String::new(),
            password: String::new(),
            phone: String::new(),
            captcha: String::new(),
            qr_key: String::new(),
            qr_url: String::new(),
            status_line: "按 F1 刷新二维码后扫码登录".to_string(),
        }
    }
}

impl LoginState {
    fn set_method(&mut self, method: LoginMethod) {
        if self.method != method {
            self.method = method;
            self.focus_index = 0;
        }
    }

    fn field_count(&self) -> usize {
        match self.method {
            LoginMethod::Qr => 2,
            LoginMethod::Username => 3,
            LoginMethod::Phone => 4,
        }
    }

    fn next_focus(&mut self) {
        let total = self.field_count();
        if total == 0 {
            return;
        }
        self.focus_index = (self.focus_index + 1) % total;
    }

    fn prev_focus(&mut self) {
        let total = self.field_count();
        if total == 0 {
            return;
        }
        self.focus_index = if self.focus_index == 0 {
            total - 1
        } else {
            self.focus_index - 1
        };
    }

    fn is_input_focused(&self) -> bool {
        match self.method {
            LoginMethod::Qr => false,
            LoginMethod::Username => self.focus_index <= 1,
            LoginMethod::Phone => self.focus_index <= 1,
        }
    }

    fn active_input_mut(&mut self) -> Option<&mut String> {
        match self.method {
            LoginMethod::Qr => None,
            LoginMethod::Username => match self.focus_index {
                0 => Some(&mut self.username),
                1 => Some(&mut self.password),
                _ => None,
            },
            LoginMethod::Phone => match self.focus_index {
                0 => Some(&mut self.phone),
                1 => Some(&mut self.captcha),
                _ => None,
            },
        }
    }

    fn push_char(&mut self, ch: char) {
        if ch.is_control() || !self.is_input_focused() {
            return;
        }
        if let Some(value) = self.active_input_mut()
            && value.chars().count() < MAX_INPUT_LEN
        {
            value.push(ch);
        }
    }

    fn pop_char(&mut self) {
        if !self.is_input_focused() {
            return;
        }
        if let Some(value) = self.active_input_mut() {
            value.pop();
        }
    }
}

fn extract_qr_key(response: &ApiResponse) -> String {
    for pointer in ["/data/unikey", "/data/uniKey", "/unikey", "/uniKey"] {
        if let Some(value) = response
            .body
            .pointer(pointer)
            .and_then(|value| value.as_str())
            && !value.trim().is_empty()
        {
            return value.to_string();
        }
    }
    String::new()
}

fn extract_qr_url(response: &ApiResponse) -> String {
    for pointer in ["/data/qrurl", "/data/qrUrl", "/qrurl", "/qrUrl"] {
        if let Some(value) = response
            .body
            .pointer(pointer)
            .and_then(|value| value.as_str())
            && !value.trim().is_empty()
        {
            return value.to_string();
        }
    }
    String::new()
}

/// 账号档案：uid / 昵称 /「我喜欢的音乐」歌单 id。
#[derive(Clone)]
pub(super) struct AccountProfile {
    pub(super) uid: String,
    pub(super) name: Option<String>,
    pub(super) liked_playlist_id: Option<String>,
}

/// 当前账号档案：`user/account`，失败时回退 `login/status`。
pub(super) async fn fetch_account_profile(
    api: &mut ApiState,
    lang: Language,
) -> Result<AccountProfile> {
    let account = match api.user_account().await {
        Ok(response) => response,
        Err(_) => api.login_status().await?,
    };
    let code = response_code(&account);
    if code != 200 {
        return Err(anyhow!(
            "{}({}): {}",
            lang_text(lang, "账号信息请求失败", "Failed to fetch account profile"),
            code,
            response_message(&account)
        ));
    }

    let uid = extract_current_user_id(&account).ok_or_else(|| {
        anyhow!(lang_text(
            lang,
            "未找到当前用户 ID",
            "Current user id not found"
        ))
    })?;

    Ok(AccountProfile {
        uid,
        name: extract_current_user_name(&account),
        liked_playlist_id: extract_liked_playlist_id(&account),
    })
}

/// 会员音质权限：先查 `vip/info/v2`，未命中再回退 `vip/info`。
pub(super) async fn fetch_vip_unlocked(api: &mut ApiState) -> bool {
    let mut unlocked = false;

    if let Ok(response) = api.vip_info_v2().await {
        unlocked = response_indicates_vip(&response);
    }

    if !unlocked && let Ok(response) = api.vip_info().await {
        unlocked = response_indicates_vip(&response);
    }

    unlocked
}

/// 扫码登录二维码：key 与二维码链接。
pub(super) struct QrLoginCode {
    pub(super) key: String,
    pub(super) url: String,
}

pub(super) async fn fetch_qr_login_code(api: &mut ApiState, lang: Language) -> Result<QrLoginCode> {
    let key_resp = api
        .login_qr_key()
        .await
        .with_context(|| lang_text(lang, "二维码 key 获取失败", "Failed to fetch QR login key"))?;

    let key = extract_qr_key(&key_resp);
    if key.is_empty() {
        bail!(lang_text(
            lang,
            "二维码 key 为空，请重试",
            "Empty QR login key, please retry"
        ));
    }

    let qr_resp = api
        .login_qr_create(&key)
        .await
        .with_context(|| lang_text(lang, "二维码创建失败", "Failed to create QR login code"))?;

    Ok(QrLoginCode {
        key,
        url: extract_qr_url(&qr_resp),
    })
}

pub(super) fn extract_current_user_id(response: &ApiResponse) -> Option<String> {
    for pointer in [
        "/profile/userId",
        "/data/profile/userId",
        "/account/id",
        "/data/account/id",
    ] {
        if let Some(value) = response.body.pointer(pointer)
            && let Some(id) = parse_value_as_string(Some(value))
            && !id.trim().is_empty()
        {
            return Some(id);
        }
    }

    None
}

pub(super) fn extract_liked_playlist_id(response: &ApiResponse) -> Option<String> {
    for pointer in [
        "/profile/playlistId",
        "/data/profile/playlistId",
        "/profile/likesPlaylistId",
        "/data/profile/likesPlaylistId",
    ] {
        if let Some(value) = response.body.pointer(pointer)
            && let Some(id) = parse_value_as_string(Some(value))
            && !id.trim().is_empty()
        {
            return Some(id);
        }
    }

    None
}

pub(super) fn extract_current_user_name(response: &ApiResponse) -> Option<String> {
    for pointer in [
        "/profile/nickname",
        "/data/profile/nickname",
        "/account/userName",
        "/data/account/userName",
    ] {
        if let Some(name) = response
            .body
            .pointer(pointer)
            .and_then(|value| value.as_str())
        {
            let name = name.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }

    None
}

fn response_indicates_vip(response: &ApiResponse) -> bool {
    let code = response
        .body
        .get("code")
        .and_then(|value| value.as_i64())
        .unwrap_or(response.status);
    if code != 200 {
        return false;
    }

    let root = response.body.get("data").unwrap_or(&response.body);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    for pointer in [
        "/redVipLevel",
        "/redplusLevel",
        "/musicPackage/vipCode",
        "/associator/vipCode",
        "/musicVipLevel",
    ] {
        if root
            .pointer(pointer)
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
            > 0
        {
            return true;
        }
    }

    for pointer in [
        "/vipStatus",
        "/musicPackage/isSign",
        "/associator/isSign",
        "/isVip",
    ] {
        if root
            .pointer(pointer)
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            return true;
        }
    }

    let music_expire = root
        .pointer("/musicPackage/expireTime")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if music_expire > now_ms {
        return true;
    }

    let associator_expire = root
        .pointer("/associator/expireTime")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if associator_expire > now_ms {
        return true;
    }

    false
}

impl App {
    pub(super) async fn handle_login_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::F(1) => {
                self.login.set_method(LoginMethod::Qr);
                self.refresh_qr_login().await;
            }
            KeyCode::F(2) => self.login.set_method(LoginMethod::Username),
            KeyCode::F(3) => self.login.set_method(LoginMethod::Phone),
            KeyCode::Tab | KeyCode::Down => self.login.next_focus(),
            KeyCode::BackTab | KeyCode::Up => self.login.prev_focus(),
            KeyCode::Enter => self.submit_login_action().await,
            KeyCode::Backspace => self.login.pop_char(),
            KeyCode::Char(ch) if matches!(ch, 'q' | 'Q') => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    let typing_username_or_password =
                        self.login.method == LoginMethod::Username && self.login.focus_index <= 1;
                    if typing_username_or_password {
                        self.login.push_char(ch);
                    } else {
                        self.should_quit = true;
                    }
                }
            }
            KeyCode::Char(ch)
                if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                self.login.push_char(ch);
            }
            _ => {}
        }
    }

    async fn refresh_vip_audio_access(&mut self) {
        let unlocked = fetch_vip_unlocked(&mut self.api).await;
        self.apply_vip_audio_access(unlocked);
    }

    pub(super) fn apply_vip_audio_access(&mut self, unlocked: bool) {
        self.vip_audio_unlocked = unlocked;
        self.set_audio_quality(self.config.audio_quality);
    }

    pub(super) fn set_audio_quality(&mut self, quality: AudioQuality) {
        let clamped = quality.clamp_for_vip(self.vip_audio_unlocked);
        if self.config.audio_quality != clamped {
            self.config.audio_quality = clamped;
            self.persist_config();
        }
    }

    pub(super) async fn logout_to_login(&mut self) {
        self.cancel_page_transition();
        self.downloads.pending_intents.invalidate_session();
        self.close_overlay();
        self.page = Page::Login;
        self.search_return_page = Page::Home;
        self.input.search_box_input.clear();
        self.settings.reset_navigation();
        self.session_cookie = None;
        self.api.clear_cookie();
        let _ = self
            .persistence
            .enqueue_latest(PersistenceKey::Session, session::clear_cookie);
        self.clear_playback_memory();
        let _ = self
            .persistence
            .enqueue_latest(PersistenceKey::PrivateRoam, private_roam::clear);
        self.browse.reset_pages();
        let _ = self.cover_fetch_tx.send(None);
        let _ = self.lyric_fetch_tx.send(None);
        self.cover_fetch_generation = self.cover_fetch_generation.wrapping_add(1);
        self.lyric_fetch_generation = self.lyric_fetch_generation.wrapping_add(1);
        self.cover_fetch_inflight_url = None;
        self.lyric_fetch_inflight_song_id = None;
        self.vip_audio_unlocked = false;
        self.config.audio_quality = self.config.audio_quality.clamp_for_vip(false);

        self.login = LoginState::default();
        self.search = SearchController::default();
        // 登出同样要作废在途拉取：它们带着上一账号的 cookie 落地，会把已清空的
        // 状态写回旧账号的数据（同 `apply_playlist_fetch` 的规则）。
        self.browse.playlist_fetch = None;
        self.browse.author_fetch = None;
        self.playlist_section_return_snapshot = None;
        self.startup.progress = 0.0;
        self.startup.started_at = None;
        self.startup.complete_started_at = None;
        self.startup.complete_requested = false;
        self.last_global_hotkey_at = None;
        self.last_content_click = None;
        self.clear_content_hits();
        self.playback.audio_player.stop();
        self.playback.now_playing = None;
        self.playback.clear_like_state();
        self.playback.clear_queue();

        self.refresh_qr_login().await;
    }

    async fn submit_login_action(&mut self) {
        match self.login.method {
            LoginMethod::Qr => {
                if self.login.focus_index == 0 {
                    self.refresh_qr_login().await;
                } else {
                    self.check_qr_status_and_login().await;
                }
            }
            LoginMethod::Username => match self.login.focus_index {
                0 | 1 => self.login.next_focus(),
                _ => self.submit_username_login().await,
            },
            LoginMethod::Phone => match self.login.focus_index {
                0 | 1 => self.login.next_focus(),
                2 => self.send_phone_captcha().await,
                _ => self.submit_phone_login().await,
            },
        }
    }

    async fn refresh_qr_login(&mut self) {
        self.qr_last_poll_at = None;

        match fetch_qr_login_code(&mut self.api, self.config.language).await {
            Ok(code) => self.apply_qr_login_code(code),
            Err(err) => self.login.status_line = format!("{err}"),
        }
    }

    /// 应用一个刚拿到的登录二维码（手动刷新与启动初始化共用）。
    pub(super) fn apply_qr_login_code(&mut self, code: QrLoginCode) {
        self.login.qr_key = code.key;
        self.login.qr_url = code.url.clone();
        self.login.status_line = if code.url.is_empty() {
            "二维码已刷新，请按 Enter 轮询状态".to_string()
        } else {
            format!("二维码已刷新: {}", truncate_text(&code.url, 48))
        };
    }

    pub(super) async fn check_qr_status_and_login(&mut self) {
        if self.login.qr_key.trim().is_empty() {
            self.login.status_line = "请先按 Enter 刷新二维码".to_string();
            return;
        }

        let response = match self.api.login_qr_check(&self.login.qr_key).await {
            Ok(response) => response,
            Err(err) => {
                self.login.status_line = format!("轮询二维码失败: {}", err);
                return;
            }
        };

        let code = response_code(&response);
        match code {
            800 => {
                self.login.status_line = "二维码已过期，已自动刷新".to_string();
                self.refresh_qr_login().await;
            }
            801 => self.login.status_line = "等待扫码".to_string(),
            802 => self.login.status_line = "已扫码，等待确认".to_string(),
            803 | 200 => self.mark_login_success("二维码登录成功").await,
            _ => {
                self.login.status_line =
                    format!("二维码状态异常({}): {}", code, response_message(&response))
            }
        }
    }

    async fn submit_username_login(&mut self) {
        let username = self.login.username.trim().to_string();
        if username.is_empty() || self.login.password.trim().is_empty() {
            self.login.status_line = "请填写用户名和密码".to_string();
            return;
        }

        let response = match self.api.login_email(&username, &self.login.password).await {
            Ok(response) => response,
            Err(err) => {
                self.login.status_line = format!("登录失败: {}", err);
                return;
            }
        };

        let code = response_code(&response);
        if code == 200 {
            let nickname = response.body["profile"]["nickname"]
                .as_str()
                .unwrap_or("用户");
            self.mark_login_success(&format!("欢迎回来，{}", nickname))
                .await;
            return;
        }

        self.login.status_line = format!("登录失败({}): {}", code, response_message(&response));
    }

    async fn send_phone_captcha(&mut self) {
        let phone = self.login.phone.trim().to_string();
        if phone.is_empty() {
            self.login.status_line = "请输入手机号".to_string();
            return;
        }

        let response = match self.api.captcha_sent(&phone).await {
            Ok(response) => response,
            Err(err) => {
                self.login.status_line = format!("验证码发送失败: {}", err);
                return;
            }
        };

        let code = response_code(&response);
        if code == 200 {
            self.login.status_line = format!("验证码已发送到 {}", phone);
            return;
        }

        self.login.status_line = format!("发送失败({}): {}", code, response_message(&response));
    }

    async fn submit_phone_login(&mut self) {
        let phone = self.login.phone.trim().to_string();
        let captcha = self.login.captcha.trim().to_string();

        if phone.is_empty() || captcha.is_empty() {
            self.login.status_line = "请填写手机号和验证码".to_string();
            return;
        }

        let response = match self.api.login_phone_captcha(&phone, &captcha).await {
            Ok(response) => response,
            Err(err) => {
                self.login.status_line = format!("手机号登录失败: {}", err);
                return;
            }
        };

        let code = response_code(&response);
        if code == 200 {
            let nickname = response.body["profile"]["nickname"]
                .as_str()
                .unwrap_or("用户");
            self.mark_login_success(&format!("欢迎回来，{}", nickname))
                .await;
            return;
        }

        self.login.status_line = format!("登录失败({}): {}", code, response_message(&response));
    }

    pub(super) async fn resolve_current_user_id(&mut self) -> Result<String> {
        if let Some(uid) = self.browse.home_sidebar.user_id.as_ref() {
            return Ok(uid.clone());
        }

        let profile = fetch_account_profile(&mut self.api, self.config.language).await?;
        self.apply_account_profile(profile);
        Ok(self.browse.home_sidebar.user_id.clone().unwrap_or_default())
    }

    /// 应用账号档案（侧边栏用户名 / uid / 我喜欢歌单 id）。
    pub(super) fn apply_account_profile(&mut self, profile: AccountProfile) {
        self.browse.home_sidebar.user_id = Some(profile.uid);
        self.browse.home_sidebar.liked_playlist_id = profile.liked_playlist_id;
        if let Some(name) = profile.name {
            self.browse.home_sidebar.user_name = name;
        }
    }

    async fn mark_login_success(&mut self, text: &str) {
        self.session_cookie = self.api.session_cookie().map(|value| value.to_string());
        if let Some(cookie) = self.session_cookie.as_deref() {
            let cookie = cookie.to_string();
            let _ = self
                .persistence
                .enqueue_latest(PersistenceKey::Session, move || {
                    session::save_cookie(&cookie)
                });
        }
        self.refresh_vip_audio_access().await;
        let _ = self.refresh_liked_song_cache().await;
        self.browse.home_sidebar = HomeSidebarState::default();
        self.playlist_section_return_snapshot = None;
        self.browse.home.status_line = text.to_string();
        // 登录后这次刷新仍走同步链路：进度条只按时间缓动，不接后台步数。
        self.startup.init.reset_steps(0, 1);
        self.begin_startup_loading(Page::Home);
        if !self.private_roam_refreshed_today() {
            let tracks = fetch_private_roam_songs(&mut self.api).await;
            self.apply_private_roam_refresh(tracks);
        }
        if let Err(err) = self.load_home_recommendations().await {
            self.browse.home.status_line = format!("{}，推荐歌单加载失败: {}", text, err);
        }
        self.finish_startup_loading();
        self.try_restore_playback_memory().await;
    }
}
