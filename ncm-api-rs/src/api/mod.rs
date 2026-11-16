/// API 模块 - 对应 Node.js 版本的 module/ 目录
///
/// 每个 API 接口拆分为独立文件，通过 `impl ApiClient` 扩展方法
/// 所有方法统一使用 `Query` 对象传参
use crate::request::{CryptoType, RequestOption};
use std::collections::HashMap;

// ---- 歌曲相关 ----
mod check_music;
mod like;
mod likelist;
mod lyric;
mod lyric_new;
mod scrobble;
mod song_chorus;
mod song_detail;
mod song_downlist;
mod song_download_url;
mod song_download_url_v1;
mod song_dynamic_cover;
mod song_like_check;
mod song_lyrics_mark;
mod song_lyrics_mark_add;
mod song_lyrics_mark_del;
mod song_lyrics_mark_user_page;
mod song_monthdownlist;
mod song_music_detail;
mod song_order_update;
mod song_purchased;
mod song_red_count;
mod song_singledownlist;
mod song_url;
mod song_url_v1;
mod song_url_v1_302;
mod song_wiki_summary;

// ---- 搜索相关 ----
mod cloudsearch;
mod search;
mod search_default;
mod search_hot;
mod search_hot_detail;
mod search_match;
mod search_multimatch;
mod search_suggest;

// ---- 听歌识曲 ----
mod audio_match;

// ---- 歌单相关 ----
mod playlist_category_list;
mod playlist_catlist;
mod playlist_cover_update;
mod playlist_create;
mod playlist_delete;
mod playlist_desc_update;
mod playlist_detail;
mod playlist_detail_dynamic;
mod playlist_detail_rcmd_get;
mod playlist_highquality_tags;
mod playlist_hot;
mod playlist_import_name_task_create;
mod playlist_import_task_status;
mod playlist_mylike;
mod playlist_name_update;
mod playlist_order_update;
mod playlist_privacy;
mod playlist_subscribe;
mod playlist_subscribers;
mod playlist_tags_update;
mod playlist_track_add;
mod playlist_track_all;
mod playlist_track_delete;
mod playlist_tracks;
mod playlist_update;
mod playlist_update_playcount;
mod playlist_video_recent;
mod user_playlist;

// ---- 歌手相关 ----
mod artist_album;
mod artist_desc;
mod artist_detail;
mod artist_detail_dynamic;
mod artist_fans;
mod artist_follow_count;
mod artist_list;
mod artist_mv;
mod artist_new_mv;
mod artist_new_song;
mod artist_songs;
mod artist_sub;
mod artist_sublist;
mod artist_top_song;
mod artist_video;
mod artists;

// ---- 专辑相关 ----
mod album;
mod album_detail;
mod album_detail_dynamic;
mod album_list;
mod album_list_style;
mod album_new;
mod album_newest;
mod album_privilege;
mod album_songsaleboard;
mod album_sub;
mod album_sublist;

// ---- 评论相关 ----
mod comment;
mod comment_album;
mod comment_dj;
mod comment_event;
mod comment_floor;
mod comment_hot;
mod comment_hug_list;
mod comment_info_list;
mod comment_like;
mod comment_music;
mod comment_mv;
mod comment_new;
mod comment_playlist;
mod comment_video;

// ---- 推荐相关 ----
mod history_recommend_songs;
mod history_recommend_songs_detail;
mod personalized;
mod personalized_djprogram;
mod personalized_mv;
mod personalized_newsong;
mod personalized_privatecontent;
mod personalized_privatecontent_list;
mod program_recommend;
mod recommend_resource;
mod recommend_songs;
mod recommend_songs_dislike;

// ---- 登录相关 ----
mod activate_init_profile;
mod login;
mod login_cellphone;
mod login_qr_check;
mod login_qr_create;
mod login_qr_key;
mod login_refresh;
mod login_status;
mod logout;
mod register_anonimous;
mod register_cellphone;

// ---- 验证相关 ----
mod captcha_sent;
mod captcha_verify;
mod cellphone_existence_check;
mod nickname_check;
mod verify_get_qr;
mod verify_qrcodestatus;

// ---- 用户相关 ----
mod follow;
mod get_userids;
mod rebind;
mod setting;
mod user_account;
mod user_audio;
mod user_binding;
mod user_bindingcellphone;
mod user_cloud;
mod user_cloud_del;
mod user_cloud_detail;
mod user_comment_history;
mod user_detail;
mod user_detail_new;
mod user_dj;
mod user_event;
mod user_follow_mixed;
mod user_followeds;
mod user_follows;
mod user_level;
mod user_medal;
mod user_mutualfollow_get;
mod user_playlist_collect;
mod user_playlist_create;
mod user_record;
mod user_replacephone;
mod user_social_status;
mod user_social_status_edit;
mod user_social_status_rcmd;
mod user_social_status_support;
mod user_subcount;
mod user_update;

// ---- 私人 FM ----
mod fm_trash;
mod personal_fm;
mod personal_fm_mode;

// ---- Banner ----
mod banner;

// ---- 签到 ----
mod daily_signin;
mod sign_happy_info;
mod signin_progress;

// ---- MV 相关 ----
mod mv_all;
mod mv_detail;
mod mv_detail_info;
mod mv_exclusive_rcmd;
mod mv_first;
mod mv_sub;
mod mv_sublist;
mod mv_url;

// ---- 电台相关 ----
mod dj_banner;
mod dj_category_excludehot;
mod dj_category_recommend;
mod dj_catelist;
mod dj_detail;
mod dj_hot;
mod dj_paygift;
mod dj_personalize_recommend;
mod dj_program;
mod dj_program_detail;
mod dj_program_toplist;
mod dj_program_toplist_hours;
mod dj_radio_hot;
mod dj_radio_top;
mod dj_recommend;
mod dj_recommend_type;
mod dj_sub;
mod dj_sublist;
mod dj_subscriber;
mod dj_today_perfered;
mod dj_toplist;
mod dj_toplist_hours;
mod dj_toplist_newcomer;
mod dj_toplist_pay;
mod dj_toplist_popular;

// ---- 相似推荐 ----
mod simi_artist;
mod simi_mv;
mod simi_playlist;
mod simi_song;
mod simi_user;

// ---- 排行榜 ----
mod top_album;
mod top_artists;
mod top_list;
mod top_mv;
mod top_playlist;
mod top_playlist_highquality;
mod top_song;
mod toplist;
mod toplist_artist;
mod toplist_detail;
mod toplist_detail_v2;

// ---- 视频相关 ----
mod video_category_list;
mod video_detail;
mod video_detail_info;
mod video_group;
mod video_group_list;
mod video_sub;
mod video_timeline_all;
mod video_timeline_recommend;
mod video_url;

// ---- 私信 ----
mod msg_comments;
mod msg_forwards;
mod msg_notices;
mod msg_private;
mod msg_private_history;
mod msg_recentcontact;

// ---- 动态 ----
mod event;
mod event_del;
mod event_forward;

// ---- 发送/分享 ----
mod send_album;
mod send_playlist;
mod send_song;
mod send_text;
mod share_resource;

// ---- 资源操作 ----
mod resource_like;

// ---- 国家编码 ----
mod countries_code_list;

// ---- 最近播放 ----
mod recent_listen_list;
mod record_recent_album;
mod record_recent_dj;
mod record_recent_playlist;
mod record_recent_song;
mod record_recent_video;
mod record_recent_voice;

// ---- 话题相关 ----
mod hot_topic;
mod topic_detail;
mod topic_detail_event_hot;
mod topic_sublist;

// ---- 曲风相关 ----
mod style_album;
mod style_artist;
mod style_detail;
mod style_list;
mod style_playlist;
mod style_preference;
mod style_song;

// ---- 数字专辑相关 ----
mod digital_album_detail;
mod digital_album_ordering;
mod digital_album_purchased;
mod digital_album_sales;

// ---- 乐谱相关 ----
mod sheet_list;
mod sheet_preview;

// ---- Mlog 相关 ----
mod mlog_music_rcmd;
mod mlog_to_video;
mod mlog_url;

// ---- 听歌足迹 ----
mod listen_data_realtime_report;
mod listen_data_report;
mod listen_data_today_song;
mod listen_data_total;
mod listen_data_year_report;

// ---- 广播电台 ----
mod broadcast_category_region_get;
mod broadcast_channel_collect_list;
mod broadcast_channel_currentinfo;
mod broadcast_channel_list;
mod broadcast_sub;

// ---- 达人/创作者 ----
mod creator_authinfo_get;
mod threshold_detail_get;

// ---- 年度报告 ----
mod summary_annual;

// ---- 音乐人相关 ----
mod musician_cloudbean;
mod musician_cloudbean_obtain;
mod musician_data_overview;
mod musician_play_trend;
mod musician_sign;
mod musician_tasks;
mod musician_tasks_new;
mod musician_vip_tasks;

// ---- UGC 百科相关 ----
mod ugc_album_get;
mod ugc_artist_get;
mod ugc_artist_search;
mod ugc_detail;
mod ugc_mv_get;
mod ugc_song_get;
mod ugc_user_devote;

// ---- 粉丝中心相关 ----
mod fanscenter_basicinfo_age_get;
mod fanscenter_basicinfo_gender_get;
mod fanscenter_basicinfo_province_get;
mod fanscenter_overview_get;
mod fanscenter_trend_list;

// ---- 声音相关 ----
mod voice_delete;
mod voice_detail;
mod voice_lyric;
mod voicelist_detail;
mod voicelist_list;
mod voicelist_list_search;
mod voicelist_search;
mod voicelist_trans;

// ---- 私人 DJ ----
mod aidj_content_rcmd;

// ---- 回忆坐标 ----
mod music_first_listen_info;

// ---- 一起听相关 ----
mod listentogether_accept;
mod listentogether_end;
mod listentogether_heatbeat;
mod listentogether_play_command;
mod listentogether_room_check;
mod listentogether_room_create;
mod listentogether_status;
mod listentogether_sync_list_command;
mod listentogether_sync_playlist_get;

// ---- VIP 相关 ----
mod vip_growthpoint;
mod vip_growthpoint_details;
mod vip_growthpoint_get;
mod vip_info;
mod vip_info_v2;
mod vip_sign;
mod vip_sign_info;
mod vip_tasks;
mod vip_timemachine;

// ---- 云贝相关 ----
mod yunbei;
mod yunbei_expense;
mod yunbei_info;
mod yunbei_rcmd_song;
mod yunbei_rcmd_song_history;
mod yunbei_receipt;
mod yunbei_sign;
mod yunbei_task_finish;
mod yunbei_tasks;
mod yunbei_tasks_todo;
mod yunbei_today;

// ---- 云盘相关 ----
mod cloud;
mod cloud_import;
mod cloud_lyric_get;
mod cloud_match;
mod cloud_upload_complete;
mod cloud_upload_token;

// ---- EAPI 工具 ----
mod eapi_decrypt;

// ---- 其他 ----
#[allow(clippy::module_inception)]
mod api;
mod batch;
mod calendar;
mod homepage_block_page;
mod homepage_dragon_ball;
mod hug_comment;
mod inner_version;
mod pl_count;
mod playmode_intelligence_list;
mod playmode_song_vector;
mod related_allvideo;
mod related_playlist;
mod starpick_comments_summary;
mod weblog;

// ---- 上传相关 ----
mod avatar_upload;
mod voice_upload;

// ============================================================
//  通用查询参数
// ============================================================

/// 通用查询参数，用于向 API 方法传递参数
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub params: HashMap<String, String>,
    pub cookie: Option<String>,
    pub real_ip: Option<String>,
    pub random_cn_ip: bool,
    pub ua: Option<String>,
    pub e_r: Option<bool>,
    pub domain: Option<String>,
}

impl Query {
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置参数
    pub fn param(mut self, key: &str, value: &str) -> Self {
        self.params.insert(key.to_string(), value.to_string());
        self
    }

    /// 设置 cookie
    pub fn cookie(mut self, cookie: &str) -> Self {
        self.cookie = Some(cookie.to_string());
        self
    }

    /// 获取参数值
    pub fn get(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(|s| s.as_str())
    }

    /// 获取参数值，若不存在则返回默认值
    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.params
            .get(key)
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    /// 获取 i64 参数，解析失败返回默认值
    ///
    /// 替代 `query.get_or("limit", "30").parse::<i64>().unwrap_or(30)` 模式
    pub fn get_i64(&self, key: &str, default: i64) -> i64 {
        match self.params.get(key) {
            Some(v) => v.parse::<i64>().unwrap_or(default),
            None => default,
        }
    }

    /// 获取 bool 参数（接受 "true"/"false"/"1"/"0"）
    ///
    /// 替代 `query.get_or("like", "true") == "true"` 模式
    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.params.get(key).map(|s| s.as_str()) {
            Some("true") | Some("1") => true,
            Some("false") | Some("0") => false,
            _ => default,
        }
    }

    /// 构造 RequestOption
    pub(crate) fn to_option(&self, crypto: CryptoType) -> RequestOption {
        RequestOption {
            crypto,
            cookie: self.cookie.clone(),
            ua: self.ua.clone(),
            real_ip: self.real_ip.clone(),
            random_cn_ip: self.random_cn_ip,
            e_r: self.e_r,
            domain: self.domain.clone(),
            check_token: false,
            cookie_overrides: Vec::new(),
        }
    }
}
