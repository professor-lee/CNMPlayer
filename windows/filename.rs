//! Windows 保留设备名处理。
//!
//! `CON`、`NUL`、`COM1` 等名字在 Windows 上指向设备而不是文件，而且只看第一个 `.` 之前、
//! 去掉尾部空格后的部分：`CON.mp3`、`nul .flac.part` 同样无法创建。标题恰好是保留名、
//! 又没有歌手与专辑段时，下载文件名主干就会撞上。

const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];

fn is_reserved(stem: &str) -> bool {
    let head = stem.split('.').next().unwrap_or("").trim_end_matches(' ');
    if RESERVED.iter().any(|name| head.eq_ignore_ascii_case(name)) {
        return true;
    }
    let mut chars = head.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let digit = chars.next();
    let rest = chars.next();
    rest.is_none()
        && (prefix.eq_ignore_ascii_case("COM") || prefix.eq_ignore_ascii_case("LPT"))
        && digit.is_some_and(|ch| ch.is_ascii_digit() || matches!(ch, '¹' | '²' | '³'))
}

/// 撞上保留名时在主干后补 `_`（`CON` → `CON_`），其余原样返回。
pub(crate) fn avoid_reserved_stem(stem: String) -> String {
    if is_reserved(&stem) {
        let head_len = stem.find('.').unwrap_or(stem.len());
        let head = stem[..head_len].trim_end_matches(' ');
        format!("{head}_{}", &stem[head_len..])
    } else {
        stem
    }
}

#[cfg(test)]
mod tests {
    use super::avoid_reserved_stem;

    #[test]
    fn reserved_device_names_get_a_suffix_case_insensitively() {
        for (input, expected) in [
            ("CON", "CON_"),
            ("nul", "nul_"),
            ("Com1", "Com1_"),
            ("LPT9", "LPT9_"),
            ("COM¹", "COM¹_"),
            ("aux.remix", "aux_.remix"),
            ("PRN ", "PRN_"),
            ("CONOUT$", "CONOUT$_"),
        ] {
            assert_eq!(avoid_reserved_stem(input.to_string()), expected, "{input}");
        }
    }

    #[test]
    fn ordinary_titles_are_untouched() {
        for input in [
            "Console",
            "CON - Artist",
            "COM10",
            "LPT",
            "NULL",
            "空",
            "COMA",
            "con-tact",
        ] {
            assert_eq!(avoid_reserved_stem(input.to_string()), input, "{input}");
        }
    }
}
