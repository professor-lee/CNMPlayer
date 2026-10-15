use super::*;

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

    pub(super) async fn refresh_vip_audio_access(&mut self) {
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

    pub(super) async fn submit_login_action(&mut self) {
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

    pub(super) async fn refresh_qr_login(&mut self) {
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

    pub(super) async fn submit_username_login(&mut self) {
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

    pub(super) async fn send_phone_captcha(&mut self) {
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

    pub(super) async fn submit_phone_login(&mut self) {
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

    pub(super) async fn mark_login_success(&mut self, text: &str) {
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
        if let Err(err) = self.load_home_recommendations().await {
            self.browse.home.status_line = format!("{}，推荐歌单加载失败: {}", text, err);
        }
        self.finish_startup_loading();
        self.try_restore_playback_memory().await;
    }
}
