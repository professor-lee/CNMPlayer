use super::*;

pub(crate) fn response_page_has_more(
    response: &ApiResponse,
    offset: usize,
    fetched: usize,
    limit: usize,
) -> bool {
    response
        .body
        .pointer("/data/more")
        .or_else(|| response.body.pointer("/more"))
        .and_then(Value::as_bool)
        .or_else(|| {
            response
                .body
                .pointer("/data/count")
                .or_else(|| response.body.pointer("/count"))
                .and_then(|value| parse_usize_value(Some(value)))
                .map(|count| offset.saturating_add(fetched) < count)
        })
        .unwrap_or(fetched >= limit)
}

pub(crate) fn response_code(response: &ApiResponse) -> i64 {
    response
        .body
        .get("code")
        .and_then(|value| value.as_i64())
        .unwrap_or(response.status)
}

pub(crate) fn response_message(response: &ApiResponse) -> String {
    if let Some(message) = response.body.get("msg").and_then(|value| value.as_str()) {
        return message.to_string();
    }
    if let Some(message) = response
        .body
        .get("message")
        .and_then(|value| value.as_str())
    {
        return message.to_string();
    }
    "未知错误".to_string()
}

pub(crate) fn parse_usize_value(value: Option<&Value>) -> Option<usize> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return Some(number as usize);
    }
    if let Some(number) = value.as_i64()
        && number >= 0
    {
        return Some(number as usize);
    }
    if let Some(text) = value.as_str() {
        return text.trim().parse::<usize>().ok();
    }
    None
}

pub(crate) fn parse_value_as_string(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str()
        && !text.trim().is_empty()
    {
        return Some(text.to_string());
    }
    if let Some(number) = value.as_i64() {
        return Some(number.to_string());
    }
    None
}

pub(crate) fn first_non_empty(value: &Value, pointers: &[&str]) -> Option<String> {
    for pointer in pointers {
        if let Some(text) = value.pointer(pointer).and_then(|item| item.as_str()) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}
