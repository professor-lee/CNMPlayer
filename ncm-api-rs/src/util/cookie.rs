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

#[cfg(test)]
mod tests;
