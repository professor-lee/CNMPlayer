/// Cookie 工具函数
use std::collections::HashMap;

/// 将 cookie 字符串解析为 HashMap
pub fn cookie_to_json(cookie: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if cookie.is_empty() {
        return map;
    }
    for item in cookie.split(';') {
        let parts: Vec<&str> = item.splitn(2, '=').collect();
        if parts.len() == 2 {
            map.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
        }
    }
    map
}

/// 将 HashMap 转换为 cookie 字符串
pub fn cookie_obj_to_string(cookie: &HashMap<String, String>) -> String {
    cookie
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("; ")
}

/// 从 cookie map 中获取指定 key 的值
pub fn get_cookie_value(cookie: &HashMap<String, String>, key: &str) -> Option<String> {
    cookie.get(key).cloned()
}

/// Set-Cookie 属性名：解析 cookie 串时它们不是 cookie。
const SET_COOKIE_ATTRIBUTES: [&str; 10] = [
    "path", "domain", "expires", "max-age", "secure", "httponly", "samesite", "priority",
    "version", "comment",
];

fn is_set_cookie_attribute(name: &str) -> bool {
    SET_COOKIE_ATTRIBUTES
        .iter()
        .any(|attribute| attribute.eq_ignore_ascii_case(name))
}

/// 客户端持有的会话 cookie：按名保存原始值（不做编解码），保持首次出现的顺序。
///
/// `epoch` 在整体替换或清空时递增；请求开始时记下它，响应回来若已变化就不合并
/// Set-Cookie，避免登出/换号之前发出的请求把旧会话写回来。
#[derive(Default)]
pub(crate) struct CookieJar {
    entries: Vec<(String, String)>,
    epoch: u64,
}

impl CookieJar {
    /// 整体替换为 cookie 串（`k=v; k2=v2`）。兼容把 Set-Cookie 原串拼接后存下的旧格式：
    /// 其中的 Path/Expires/Max-Age 等属性会被丢弃，不再当作 cookie 发送。
    pub(crate) fn replace(&mut self, cookie: &str) {
        self.entries.clear();
        self.epoch = self.epoch.wrapping_add(1);
        for item in cookie.split(';') {
            if let Some((name, value)) = item.split_once('=') {
                let name = name.trim();
                if !name.is_empty() && !is_set_cookie_attribute(name) {
                    self.upsert(name, value.trim());
                }
            }
        }
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 序列化为 Cookie 请求头格式（原始值）。
    pub(crate) fn header(&self) -> String {
        self.entries
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// 按浏览器语义合并一条 Set-Cookie：`Max-Age<=0` 或已过期的 `Expires` 表示删除。
    pub(crate) fn apply_set_cookie(&mut self, set_cookie: &str) {
        let mut parts = set_cookie.split(';');
        let Some((name, value)) = parts.next().and_then(|pair| pair.split_once('=')) else {
            return;
        };
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let expired = parts.any(|attribute| {
            let Some((key, val)) = attribute.split_once('=') else {
                return false;
            };
            let (key, val) = (key.trim(), val.trim());
            if key.eq_ignore_ascii_case("max-age") {
                val.parse::<i64>().is_ok_and(|seconds| seconds <= 0)
            } else if key.eq_ignore_ascii_case("expires") {
                chrono::DateTime::parse_from_rfc2822(val).is_ok_and(|at| at <= chrono::Utc::now())
            } else {
                false
            }
        });
        if expired {
            self.entries.retain(|(existing, _)| existing != name);
        } else {
            self.upsert(name, value.trim());
        }
    }

    fn upsert(&mut self, name: &str, value: &str) {
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| existing == name)
        {
            Some(entry) => entry.1 = value.to_string(),
            None => self.entries.push((name.to_string(), value.to_string())),
        }
    }
}

/// 只列 cookie 名，避免 Debug 输出泄露 MUSIC_U 等会话凭据。
impl std::fmt::Debug for CookieJar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieJar")
            .field(
                "names",
                &self
                    .entries
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .field("epoch", &self.epoch)
            .finish()
    }
}

#[cfg(test)]
mod tests;
