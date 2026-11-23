//! 运行状态与报错的唯一出口。应用内（主程序与全屏页）不显示任何状态或报错文字。
//!
//! - [`info`]：状态信息只写日志（`Player.log`，info 级）。
//! - [`error`]：报错写日志（error 级），同时记入本次运行的报错清单；
//!   退出并恢复终端后由 [`print_error_report`] 输出到主终端。
//!
//! 报错清单按文案去重并计数，条目数有上限，长时间运行反复出同一错误也不会无限增长。

use crate::data::config::Language;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// 报错清单最多保留的不同文案条数；超出部分只计数。
const MAX_DISTINCT_ERRORS: usize = 64;

const LOG_TARGET: &str = "status";

struct ErrorReport {
    /// 首次出现顺序；同一文案重复出现只累加次数。
    entries: Vec<(String, usize)>,
    /// 超出条数上限而未单独列出的报错次数。
    dropped: usize,
}

static ERRORS: Mutex<ErrorReport> = Mutex::new(ErrorReport {
    entries: Vec::new(),
    dropped: 0,
});

/// 界面语言；配置载入前按中文。
static ENGLISH: AtomicBool = AtomicBool::new(false);

/// 退出报告与不持有 `App` 的报错文案（下载任务、分页拉取等）按此选择语言；
/// 启动载入配置后设置，运行中切换界面语言时同步。
pub(crate) fn set_language(language: Language) {
    ENGLISH.store(matches!(language, Language::En), Ordering::Relaxed);
}

/// 按当前界面语言选文案，供拿不到 `Config` 的报错路径使用。
pub(crate) fn localized<'a>(zh: &'a str, en: &'a str) -> &'a str {
    if ENGLISH.load(Ordering::Relaxed) {
        en
    } else {
        zh
    }
}

/// 状态信息：只写日志。
pub(crate) fn info(text: impl AsRef<str>) {
    log::info!(target: LOG_TARGET, "{}", text.as_ref());
}

/// 报错：写日志，并在本次运行结束后输出到主终端。
pub(crate) fn error(text: impl AsRef<str>) {
    let text = text.as_ref();
    log::error!(target: LOG_TARGET, "{text}");
    let mut report = ERRORS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((_, count)) = report.entries.iter_mut().find(|(seen, _)| seen == text) {
        *count += 1;
    } else if report.entries.len() < MAX_DISTINCT_ERRORS {
        report.entries.push((text.to_string(), 1));
    } else {
        report.dropped += 1;
    }
}

/// 把本次运行的报错写到 stdout（fd 2 已重定向到日志文件）。须在离开备用屏幕之后调用；
/// 没有报错时不输出任何内容。
pub(crate) fn print_error_report(log_path: &Path) {
    let report = ERRORS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if report.entries.is_empty() && report.dropped == 0 {
        return;
    }
    let english = ENGLISH.load(Ordering::Relaxed);
    let mut out = std::io::stdout().lock();
    let header = if english {
        format!(
            "CNMPlayer reported errors during this run (log: {}):",
            log_path.display()
        )
    } else {
        format!(
            "CNMPlayer 本次运行出现以下报错（日志：{}）：",
            log_path.display()
        )
    };
    let _ = writeln!(out, "{header}");
    for (text, count) in &report.entries {
        if *count > 1 {
            let _ = writeln!(out, "  - {text} (×{count})");
        } else {
            let _ = writeln!(out, "  - {text}");
        }
    }
    if report.dropped > 0 {
        let more = if english {
            format!("  … {} more, see the log", report.dropped)
        } else {
            format!("  … 另有 {} 条，见日志", report.dropped)
        };
        let _ = writeln!(out, "{more}");
    }
    let _ = out.flush();
}
