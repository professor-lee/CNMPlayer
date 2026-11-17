/// 请求模块 - 对应 Node.js 版本的 util/request.js
///
/// 核心功能：构造加密请求、Cookie 管理、UA 伪装
use crate::crypto;
use crate::error::{NcmError, Result};
use crate::util::config::*;
use crate::util::cookie::{cookie_obj_to_string, cookie_to_json};
use crate::util::device::{generate_device_id, generate_wnmcid, random_hex};
use crate::util::ip::generate_random_chinese_ip;

use http::header::{COOKIE, HeaderMap, HeaderValue, REFERER, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);

/// 上游获取易盾 token 的超时
const ANTI_CHEAT_TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
/// 易盾 JSONP 回包：null([状态码,时间戳,"token"])
static ANTI_CHEAT_TOKEN_REGEX: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r#"null\(\[(\d+),\d+,"([^"]+)"\]\)"#).unwrap());

/// 特殊状态码集合（视为 200）
static SPECIAL_STATUS_CODES: LazyLock<std::collections::HashSet<i64>> =
    LazyLock::new(|| [201, 302, 400, 502, 800, 801, 802, 803].into());

/// 全局设备 ID（进程生命周期内固定）
static DEVICE_ID: LazyLock<String> = LazyLock::new(generate_device_id);
/// 全局 WNMCID
static WNMCID: LazyLock<String> = LazyLock::new(generate_wnmcid);

/// xeapi 进程级状态：服务端公钥与服务端下发的会话（与上游的全局变量一致）
struct XeapiState {
    public_key: Option<crypto::XeapiPublicKey>,
    session_id: String,
    session_key: String,
}

static XEAPI_STATE: std::sync::Mutex<XeapiState> = std::sync::Mutex::new(XeapiState {
    public_key: None,
    session_id: String::new(),
    session_key: String::new(),
});

/// Cookie Domain 移除正则（编译一次，全局复用）
static DOMAIN_REGEX: LazyLock<regex_lite::Regex> =
    LazyLock::new(|| regex_lite::Regex::new(r"\s*Domain=[^;]+;?").unwrap());

/// 安全创建 HeaderValue，无效字符会被过滤
fn header_value(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| {
        // 过滤掉非 ASCII 可见字符
        let safe: String = s
            .chars()
            .filter(|c| c.is_ascii() && !c.is_ascii_control())
            .collect();
        HeaderValue::from_str(&safe).unwrap_or_else(|_| HeaderValue::from_static(""))
    })
}

/// 非 API 网关的直连请求（易盾 token、NOS 上传、网页抓取、听歌识曲）的基础请求头。
///
/// cyper 只在请求未带同名头时才补注入 client 的默认头，所以库发出的每个请求都显式
/// 设置 UA 与 Referer，请求特征不随调用方 client 的配置变化。
pub(crate) fn direct_request_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(choose_user_agent("weapi", "pc")),
    );
    headers.insert(REFERER, HeaderValue::from_static(WEB_REFERER));
    headers
}

fn api_endpoint(uri: &str) -> Result<&str> {
    uri.strip_prefix("/api/")
        .filter(|endpoint| !endpoint.is_empty())
        .ok_or_else(|| {
            NcmError::InvalidParam("API URI must start with /api/ and name an endpoint".to_string())
        })
}

/// 加密回包解密失败时与上游一致退回原文：服务端的错误回包常是未加密的明文 JSON。
fn decode_response_body(bytes: &[u8], encrypted: bool) -> Result<Value> {
    if encrypted {
        crypto::eapi_res_decrypt_bytes(bytes)
            .or_else(|error| serde_json::from_slice(bytes).map_err(|_| error))
    } else {
        serde_json::from_slice(bytes)
            .map_err(|source| NcmError::response_decode("plain", bytes.len(), source))
    }
}

fn response_from_body(status_code: i64, body: Value, cookie: Vec<String>) -> Result<ApiResponse> {
    let mut status = body
        .get("code")
        .and_then(|c| {
            c.as_i64()
                .or_else(|| c.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(status_code);

    // Preserve protocol-specific status semantics and the original body.
    if SPECIAL_STATUS_CODES.contains(&status) {
        status = 200;
    }
    if !(100..600).contains(&status) {
        status = 400;
    }

    let answer = ApiResponse {
        status,
        body,
        cookie,
    };
    if status == 200 {
        Ok(answer)
    } else {
        let msg = answer
            .body
            .get("msg")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown error")
            .to_string();
        Err(NcmError::from_api(status, msg))
    }
}

/// 加密类型
#[derive(Debug, Clone, PartialEq, Default)]
pub enum CryptoType {
    Weapi,
    #[default]
    Eapi,
    Linuxapi,
    /// 安卓客户端新协议（上游 xeapi）
    Xeapi,
    Api, // 明文
}

impl CryptoType {
    pub fn as_str(&self) -> &str {
        match self {
            CryptoType::Weapi => "weapi",
            CryptoType::Eapi => "eapi",
            CryptoType::Linuxapi => "linuxapi",
            CryptoType::Xeapi => "xeapi",
            CryptoType::Api => "api",
        }
    }
}

impl From<&str> for CryptoType {
    fn from(s: &str) -> Self {
        match s {
            "weapi" => CryptoType::Weapi,
            "linuxapi" => CryptoType::Linuxapi,
            "xeapi" => CryptoType::Xeapi,
            "api" => CryptoType::Api,
            _ => CryptoType::Eapi,
        }
    }
}

/// 请求选项
#[derive(Debug, Clone, Default)]
pub struct RequestOption {
    pub crypto: CryptoType,
    pub cookie: Option<String>,
    pub ua: Option<String>,
    pub real_ip: Option<String>,
    pub random_cn_ip: bool,
    pub e_r: Option<bool>,
    pub domain: Option<String>,
    /// 为 true 时请求前实时获取易盾反作弊 token，放入 X-antiCheatToken
    pub check_token: bool,
    /// 覆盖请求 cookie 中的字段（如 scrobble 的 os=osx、vivid 的安卓客户端版本）
    pub cookie_overrides: Vec<(&'static str, &'static str)>,
}

/// API 响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    pub status: i64,
    pub body: Value,
    #[serde(default)]
    pub cookie: Vec<String>,
}

/// API 客户端
#[derive(Debug, Clone)]
pub struct ApiClient {
    pub(crate) client: cyper::Client,
    cookie: Option<String>,
    anonymous_token: Option<String>,
    /// 自定义设备 ID，None 则使用全局默认值
    device_id: Option<String>,
}

impl ApiClient {
    /// 使用外部提供的 Client 创建 API 客户端
    pub fn new(cookie: Option<String>, client: cyper::Client) -> Self {
        Self {
            client,
            cookie,
            anonymous_token: None,
            device_id: None,
        }
    }

    /// 设置 cookie
    pub fn set_cookie(&mut self, cookie: String) {
        self.cookie = Some(cookie);
    }

    /// 设置匿名 token
    pub fn set_anonymous_token(&mut self, token: String) {
        self.anonymous_token = Some(token);
    }

    /// 设置自定义设备 ID
    ///
    /// 不同客户端实例可使用不同设备 ID，避免共享全局 ID 触发风控
    pub fn set_device_id(&mut self, device_id: String) {
        self.device_id = Some(device_id);
    }

    /// 获取当前设备 ID（自定义优先，否则使用全局默认）
    fn get_device_id(&self) -> &str {
        self.device_id.as_deref().unwrap_or(&DEVICE_ID)
    }

    /// 发起 API 请求；HTTP 发送到完整响应 body 共用 30 秒 deadline。
    pub async fn request(
        &self,
        uri: &str,
        data: Value,
        options: RequestOption,
    ) -> Result<ApiResponse> {
        self.request_with_timeout(uri, data, options, NETWORK_TIMEOUT)
            .await
    }

    async fn request_with_timeout(
        &self,
        uri: &str,
        data: Value,
        options: RequestOption,
        timeout: Duration,
    ) -> Result<ApiResponse> {
        let endpoint = api_endpoint(uri)?;
        if !data.is_object() {
            return Err(NcmError::InvalidParam(
                "API request data must be a JSON object".to_string(),
            ));
        }
        let mut headers = HeaderMap::new();
        // weapi 下面会换成自己的 Referer；其余加密方式沿用网页端 Referer。
        headers.insert(REFERER, HeaderValue::from_static(WEB_REFERER));
        // 上游每次实时获取、不缓存；获取失败时与上游一致发送空 token
        let anti_cheat_token = if options.check_token {
            Some(self.anti_cheat_token().await)
        } else {
            None
        };

        // IP 伪装
        let ip = options.real_ip.clone().or_else(|| {
            if options.random_cn_ip {
                Some(generate_random_chinese_ip())
            } else {
                None
            }
        });

        if let Some(ref ip) = ip
            && let (Ok(real_ip), Ok(fwd)) = (HeaderValue::from_str(ip), HeaderValue::from_str(ip))
        {
            headers.insert("X-Real-IP", real_ip);
            headers.insert("X-Forwarded-For", fwd);
        }

        // Cookie 处理
        let cookie_str = options
            .cookie
            .as_deref()
            .or(self.cookie.as_deref())
            .unwrap_or("");
        let mut cookie_map = cookie_to_json(cookie_str);
        for (key, value) in &options.cookie_overrides {
            cookie_map.insert((*key).to_string(), (*value).to_string());
        }

        // 注入必要的 cookie 字段
        let ntes_nuid = random_hex(32);
        let os = get_os_config(cookie_map.get("os").map(|s| s.as_str()).unwrap_or("pc"));
        let now_ts = chrono::Utc::now().timestamp_millis().to_string();

        cookie_map
            .entry("__remember_me".to_string())
            .or_insert_with(|| "true".to_string());
        cookie_map
            .entry("ntes_kaola_ad".to_string())
            .or_insert_with(|| "1".to_string());
        cookie_map
            .entry("_ntes_nnid".to_string())
            .or_insert_with(|| format!("{},{}", ntes_nuid, now_ts));
        cookie_map
            .entry("_ntes_nuid".to_string())
            .or_insert(ntes_nuid);
        cookie_map
            .entry("WNMCID".to_string())
            .or_insert_with(|| WNMCID.clone());
        cookie_map
            .entry("WEVNSM".to_string())
            .or_insert_with(|| "1.0.0".to_string());
        cookie_map
            .entry("osver".to_string())
            .or_insert_with(|| os.osver.to_string());
        cookie_map
            .entry("deviceId".to_string())
            .or_insert_with(|| self.get_device_id().to_string());
        cookie_map
            .entry("os".to_string())
            .or_insert_with(|| os.os.to_string());
        cookie_map
            .entry("channel".to_string())
            .or_insert_with(|| os.channel.to_string());
        cookie_map
            .entry("appver".to_string())
            .or_insert_with(|| os.appver.to_string());

        // 非登录接口注入 NMTID
        if !uri.contains("login") {
            cookie_map
                .entry("NMTID".to_string())
                .or_insert_with(|| random_hex(8));
        }

        // 未登录时注入匿名 token
        if !cookie_map.contains_key("MUSIC_U")
            && let Some(ref token) = self.anonymous_token
        {
            cookie_map
                .entry("MUSIC_A".to_string())
                .or_insert_with(|| token.clone());
        }

        headers.insert(COOKIE, header_value(&cookie_obj_to_string(&cookie_map)));

        // 确定加密方式
        let crypto_type = if options.crypto == CryptoType::Eapi && !ENCRYPT {
            CryptoType::Api
        } else {
            options.crypto.clone()
        };

        // 与上游一致：所有加密方式都带 e_r；只有 eapi/weapi 的回包会按 e_r 加密。
        let mut data = data;
        let e_r = options
            .e_r
            .or_else(|| data.get("e_r").and_then(Value::as_bool))
            .unwrap_or(ENCRYPT_RESPONSE);
        data["e_r"] = Value::Bool(e_r);
        let encrypted_response = matches!(crypto_type, CryptoType::Eapi | CryptoType::Weapi) && e_r;

        let url: String;
        let encrypt_data: HashMap<String, String>;
        let domain = options.domain.as_deref().unwrap_or("");

        let csrf_token = cookie_map.get("__csrf").cloned().unwrap_or_default();

        match crypto_type {
            CryptoType::Weapi => {
                let ref_domain = if domain.is_empty() { DOMAIN } else { domain };
                headers.insert(REFERER, header_value(ref_domain));
                let ua = options
                    .ua
                    .as_deref()
                    .unwrap_or_else(|| choose_user_agent("weapi", "pc"));
                headers.insert(USER_AGENT, header_value(ua));
                if let Some(token) = &anti_cheat_token {
                    headers.insert("X-antiCheatToken", header_value(token));
                }

                data["csrf_token"] = Value::String(csrf_token);
                encrypt_data = crypto::weapi(&data);
                url = format!("{}/weapi/{}", ref_domain, endpoint);
            }
            CryptoType::Linuxapi => {
                let ua = options
                    .ua
                    .as_deref()
                    .unwrap_or_else(|| choose_user_agent("linuxapi", "linux"));
                headers.insert(USER_AGENT, header_value(ua));

                let ref_domain = if domain.is_empty() { DOMAIN } else { domain };
                let linux_data = serde_json::json!({
                    "method": "POST",
                    "url": format!("{}{}", ref_domain, uri),
                    "params": data,
                });
                encrypt_data = crypto::linuxapi(&linux_data);
                url = format!("{}/api/linux/forward", ref_domain);
            }
            CryptoType::Xeapi => {
                let device_id = cookie_map.get("deviceId").cloned().unwrap_or_default();
                let public_key = self.xeapi_public_key(&device_id, timeout).await?;

                // xeapi 只认安卓客户端身份：非安卓 cookie 换成上游的默认安卓版本。
                let is_android = cookie_map.get("os").map(String::as_str) == Some("android");
                let android_value = |key: &str, fallback: &str| {
                    is_android
                        .then(|| cookie_map.get(key).filter(|value| !value.is_empty()))
                        .flatten()
                        .cloned()
                        .unwrap_or_else(|| fallback.to_string())
                };
                let appver = android_value("appver", XEAPI_DEFAULT_APPVER);
                let osver = android_value("osver", XEAPI_DEFAULT_OSVER);
                let buildver = cookie_map.get("buildver").cloned().unwrap_or_else(|| {
                    chrono::Utc::now().timestamp_millis().to_string()[..10].to_string()
                });
                let s_device_id = cookie_map
                    .get("sDeviceId")
                    .cloned()
                    .unwrap_or_else(|| device_id.clone());

                let ua = options.ua.as_deref().unwrap_or(XEAPI_USER_AGENT);
                headers.insert(USER_AGENT, header_value(ua));
                headers.insert("X-Client-Enc-State", HeaderValue::from_static("ENCRYPTED"));
                headers.insert("x-aeapi", HeaderValue::from_static("true"));
                headers.insert("x-deviceid", header_value(&device_id));
                headers.insert("x-os", HeaderValue::from_static("android"));
                headers.insert("x-osver", header_value(&osver));
                headers.insert("x-appver", header_value(&appver));
                headers.insert("x-sdeviceid", header_value(&s_device_id));
                headers.insert("x-buildver", header_value(&buildver));
                if let Some(music_u) = cookie_map.get("MUSIC_U") {
                    headers.insert("x-music-u", header_value(music_u));
                }
                if let Some(token) = &anti_cheat_token {
                    headers.insert("X-antiCheatToken", header_value(token));
                }

                let mut xeapi_cookie = cookie_map.clone();
                xeapi_cookie.insert("os".to_string(), "android".to_string());
                xeapi_cookie.insert("osver".to_string(), osver);
                xeapi_cookie.insert("appver".to_string(), appver);
                xeapi_cookie.insert("buildver".to_string(), buildver);
                xeapi_cookie.insert("sDeviceId".to_string(), s_device_id);
                headers.insert(COOKIE, header_value(&cookie_obj_to_string(&xeapi_cookie)));

                let session = {
                    let state = XEAPI_STATE
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    (state.session_id.clone(), state.session_key.clone())
                };
                let session = (!session.0.is_empty() && !session.1.is_empty())
                    .then_some((session.0.as_str(), session.1.as_str()));
                encrypt_data = crypto::xeapi(&data, &public_key, session, "android")?;

                let xeapi_domain = if domain.is_empty() {
                    XEAPI_DOMAIN
                } else {
                    domain
                };
                url = format!("{}/xeapi/{}", xeapi_domain, endpoint);
            }
            CryptoType::Eapi | CryptoType::Api => {
                // 构造 eapi header cookie
                let now_secs = chrono::Utc::now().timestamp().to_string();
                let request_id = format!(
                    "{}_{:04}",
                    chrono::Utc::now().timestamp_millis(),
                    rand::random::<u16>() % 1000
                );

                let mut header_map: HashMap<String, String> = HashMap::new();
                header_map.insert(
                    "osver".to_string(),
                    cookie_map.get("osver").cloned().unwrap_or_default(),
                );
                header_map.insert(
                    "deviceId".to_string(),
                    cookie_map.get("deviceId").cloned().unwrap_or_default(),
                );
                header_map.insert(
                    "os".to_string(),
                    cookie_map.get("os").cloned().unwrap_or_default(),
                );
                header_map.insert(
                    "appver".to_string(),
                    cookie_map.get("appver").cloned().unwrap_or_default(),
                );
                header_map.insert(
                    "versioncode".to_string(),
                    cookie_map
                        .get("versioncode")
                        .cloned()
                        .unwrap_or_else(|| "140".to_string()),
                );
                header_map.insert(
                    "mobilename".to_string(),
                    cookie_map.get("mobilename").cloned().unwrap_or_default(),
                );
                header_map.insert(
                    "buildver".to_string(),
                    cookie_map
                        .get("buildver")
                        .cloned()
                        .unwrap_or_else(|| now_secs[..10].to_string()),
                );
                header_map.insert(
                    "resolution".to_string(),
                    cookie_map
                        .get("resolution")
                        .cloned()
                        .unwrap_or_else(|| "1920x1080".to_string()),
                );
                header_map.insert("__csrf".to_string(), csrf_token.clone());
                header_map.insert(
                    "channel".to_string(),
                    cookie_map.get("channel").cloned().unwrap_or_default(),
                );
                header_map.insert("requestId".to_string(), request_id);

                if let Some(token) = &anti_cheat_token {
                    header_map.insert("X-antiCheatToken".to_string(), token.clone());
                }

                if let Some(music_u) = cookie_map.get("MUSIC_U") {
                    header_map.insert("MUSIC_U".to_string(), music_u.clone());
                }
                if let Some(music_a) = cookie_map.get("MUSIC_A") {
                    header_map.insert("MUSIC_A".to_string(), music_a.clone());
                }

                let header_cookie_str = header_map
                    .iter()
                    .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("; ");

                headers.insert(COOKIE, header_value(&header_cookie_str));

                let ua = options.ua.as_deref().unwrap_or_else(|| {
                    if cookie_map.get("os").map(String::as_str) == Some("osx") {
                        OSX_EAPI_USER_AGENT
                    } else {
                        choose_user_agent("api", "iphone")
                    }
                });
                headers.insert(USER_AGENT, header_value(ua));

                if crypto_type == CryptoType::Eapi {
                    // 注入 header
                    let header_value = serde_json::to_value(&header_map).unwrap();
                    data["header"] = header_value;

                    let eapi_domain = if domain.is_empty() {
                        EAPI_DOMAIN
                    } else {
                        domain
                    };
                    encrypt_data = crypto::eapi(uri, &data);
                    url = format!("{}/eapi/{}", eapi_domain, endpoint);
                } else {
                    // api 明文
                    encrypt_data = if let Value::Object(map) = &data {
                        map.iter()
                            .map(|(k, v)| {
                                (
                                    k.clone(),
                                    match v {
                                        Value::String(s) => s.clone(),
                                        _ => v.to_string(),
                                    },
                                )
                            })
                            .collect()
                    } else {
                        HashMap::new()
                    };
                    let api_domain = if domain.is_empty() {
                        API_DOMAIN
                    } else {
                        domain
                    };
                    url = format!("{}{}", api_domain, uri);
                }
            }
        }

        // 构造 POST body
        let body = serde_urlencoded::to_string(&encrypt_data)
            .map_err(|e| NcmError::Unknown(e.to_string()))?;

        // 设置 Content-Type
        let content_type = if crypto_type == CryptoType::Xeapi {
            "application/x-www-form-urlencoded;charset=utf-8"
        } else {
            "application/x-www-form-urlencoded"
        };
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static(content_type),
        );

        // One budget covers connection, response headers and the entire body.
        // Decode/business errors remain outside the network timeout boundary.
        let (status_code, resp_cookies, xeapi_session, bytes) =
            compio::time::timeout(timeout, async {
                let response = self
                    .client
                    .post(&url)?
                    .headers(headers)
                    .body(body)
                    .send()
                    .await?;

                // 处理响应 cookie
                let resp_cookies: Vec<String> = response
                    .headers()
                    .get_all("set-cookie")
                    .iter()
                    .filter_map(|v| v.to_str().ok())
                    .map(|s| {
                        // 移除 Domain 属性
                        DOMAIN_REGEX.replace_all(s, "").to_string()
                    })
                    .collect();

                // xeapi 服务端下发的会话，之后的请求用 sskey 作动态密钥
                let header_text = |name: &str| {
                    response
                        .headers()
                        .get(name)
                        .and_then(|value| value.to_str().ok())
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                };
                let xeapi_session = header_text("x-encr-ssid").zip(header_text("x-encr-sskey"));

                let status_code = response.status().as_u16() as i64;
                let bytes = response.bytes().await?;
                Ok::<_, NcmError>((status_code, resp_cookies, xeapi_session, bytes))
            })
            .await
            .map_err(|_| NcmError::Timeout { timeout })??;
        let body = if crypto_type == CryptoType::Xeapi {
            if let Some((session_id, session_key)) = xeapi_session {
                let mut state = XEAPI_STATE
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                state.session_id = session_id;
                state.session_key = session_key;
            }
            crypto::xeapi_res_decrypt(&bytes)
                .or_else(|error| serde_json::from_slice(&bytes).map_err(|_| error))?
        } else {
            decode_response_body(&bytes, encrypted_response)?
        };
        response_from_body(status_code, body, resp_cookies)
    }

    /// 易盾反作弊 token（上游 register_checktoken_v3）。
    ///
    /// 上游的 v2 需在浏览器环境运行易盾 Watchman JS，原生客户端无法复现；
    /// v3 同为易盾为网易云签发的实时 token，失败时返回空串。
    async fn anti_cheat_token(&self) -> String {
        let fetch = async {
            let body = self
                .client
                .get(ANTI_CHEAT_TOKEN_URL)?
                .headers(direct_request_headers())
                .send()
                .await?
                .text()
                .await?;
            Ok::<_, NcmError>(body)
        };
        let Ok(Ok(body)) = compio::time::timeout(ANTI_CHEAT_TOKEN_TIMEOUT, fetch).await else {
            return String::new();
        };
        ANTI_CHEAT_TOKEN_REGEX
            .captures(&body)
            .filter(|captures| &captures[1] == "200")
            .map(|captures| captures[2].to_string())
            .unwrap_or_default()
    }

    /// 取 xeapi 公钥（上游 register_xeapikey）：进程内缓存，首次使用时向
    /// `/api/bsr/sk/get` 注册并校验回包签名。
    async fn xeapi_public_key(
        &self,
        device_id: &str,
        timeout: Duration,
    ) -> Result<crypto::XeapiPublicKey> {
        if let Some(key) = XEAPI_STATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .public_key
            .clone()
        {
            return Ok(key);
        }

        let nonce: String = (0..16)
            .map(|_| char::from(b'0' + rand::random_range(0..10u8)))
            .collect();
        let timestamp = chrono::Utc::now().timestamp_millis().to_string();
        let signature = crypto::xeapi_sign(&timestamp, &nonce);
        let form = [
            ("appVersion", XEAPI_REGISTER_APPVER),
            ("currentKeyVersion", ""),
            ("deviceId", device_id),
            ("nonce", &nonce),
            ("os", "android"),
            ("requestType", "active"),
            ("signature", &signature),
            ("t1", ""),
            ("t2", ""),
            ("timestamp", &timestamp),
            ("uid", ""),
        ];
        let body =
            serde_urlencoded::to_string(form).map_err(|e| NcmError::Unknown(e.to_string()))?;
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(XEAPI_USER_AGENT));
        headers.insert(REFERER, HeaderValue::from_static(WEB_REFERER));
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        if !device_id.is_empty() {
            headers.insert(
                COOKIE,
                header_value(&format!("deviceId={}", urlencoding::encode(device_id))),
            );
        }

        let bytes = compio::time::timeout(timeout, async {
            let response = self
                .client
                .post(format!("{API_DOMAIN}/api/bsr/sk/get"))?
                .headers(headers)
                .body(body)
                .send()
                .await?;
            Ok::<_, NcmError>(response.bytes().await?)
        })
        .await
        .map_err(|_| NcmError::Timeout { timeout })??;
        let response: Value = serde_json::from_slice(&bytes)
            .map_err(|source| NcmError::response_decode("xeapi key", bytes.len(), source))?;

        let code = response.get("code").and_then(Value::as_i64).unwrap_or(0);
        let data = &response["data"];
        let encrypted = data["encryptedData"].as_str().filter(|_| code == 200);
        let Some(encrypted) = encrypted else {
            return Err(NcmError::Api {
                code,
                msg: "xeapi public key request failed".to_string(),
            });
        };
        let response_timestamp = match &data["timestamp"] {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        if data["signature"].as_str() != Some(&crypto::xeapi_sign(&response_timestamp, &nonce)) {
            return Err(NcmError::Crypto(
                "xeapi public key response signature mismatch".to_string(),
            ));
        }

        let key = crypto::xeapi_decrypt_public_key(encrypted)?;
        XEAPI_STATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .public_key = Some(key.clone());
        Ok(key)
    }
}

#[cfg(test)]
mod tests;
