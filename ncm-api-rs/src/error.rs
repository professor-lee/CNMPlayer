/// 错误类型定义
use thiserror::Error;

#[derive(Error, Debug)]
pub enum NcmError {
    /// HTTP 请求失败（网络错误、DNS 解析失败等）
    #[error("HTTP request failed: {0}")]
    Http(#[from] cyper::Error),

    /// API 业务错误（网易云返回非 200 状态码）
    #[error("API error (code={code}): {msg}")]
    Api { code: i64, msg: String },

    /// 需要登录（API 返回 301）
    #[error("Authentication required: {0}")]
    AuthRequired(String),

    /// 参数错误（缺少必要参数或格式不正确）
    #[error("Invalid parameter: {0}")]
    InvalidParam(String),

    /// 加密/解密错误
    #[error("Crypto error: {0}")]
    Crypto(String),

    /// JSON 序列化/反序列化错误
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// 请求超时
    #[error("Request timeout: {0}")]
    Timeout(String),

    /// 触发风控/限流（API 返回 503 或 IP 高频）
    #[error("Rate limited: {0}")]
    RateLimited(String),

    /// 其他错误
    #[error("{0}")]
    Unknown(String),
}

impl NcmError {
    /// 从 API 状态码和消息构造合适的错误类型
    pub fn from_api(code: i64, msg: String) -> Self {
        match code {
            301 => NcmError::AuthRequired(if msg.is_empty() {
                "需要登录".to_string()
            } else {
                msg
            }),
            400 => NcmError::InvalidParam(msg),
            503 => NcmError::RateLimited(msg),
            _ => NcmError::Api { code, msg },
        }
    }
}

pub type Result<T> = std::result::Result<T, NcmError>;
