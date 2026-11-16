use super::*;

/// 主程序列表必须恰好列出每个槽位一次：漏掉的槽位改不了，重复的会让同一键出现两行。
#[test]
fn host_view_lists_every_slot_exactly_once() {
    let mut seen = [0usize; SLOTS.len()];
    for row in key_rows(KeyView::Host) {
        if let KeyRow::Slot(index) = row {
            seen[*index] += 1;
        }
    }
    assert!(seen.iter().all(|&count| count == 1), "{seen:?}");
}

/// 全屏页只列全屏页里生效的键：宿主专用的槽位与固定键都不出现，全屏可用的一个不少。
#[test]
fn fullscreen_view_lists_exactly_the_fullscreen_keys() {
    let rows = key_rows(KeyView::Fullscreen);
    for row in rows {
        if let Some(scope) = row.scope() {
            assert!(scope.fullscreen(), "{row:?} 不在全屏页生效");
        }
    }
    for (index, slot) in SLOTS.iter().enumerate() {
        assert_eq!(
            rows.contains(&KeyRow::Slot(index)),
            slot.scope.fullscreen(),
            "槽位 {index}"
        );
    }
}

/// 没有内容的分组不画标题；每个标题后面至少跟一条条目。
#[test]
fn every_header_is_followed_by_an_entry() {
    for view in [KeyView::Host, KeyView::Fullscreen] {
        let rows = key_rows(view);
        for (index, row) in rows.iter().enumerate() {
            if row.is_header() {
                assert!(
                    rows.get(index + 1).is_some_and(|next| !next.is_header()),
                    "{view:?} 第 {index} 行是空分组"
                );
            }
        }
    }
}

/// 焦点移动跳过分组标题，首尾循环；从任何条目出发走一圈都能回到原处。
#[test]
fn focus_steps_skip_headers_and_wrap() {
    let rows = [
        KeyRow::Header(KeyGroup::General),
        KeyRow::Slot(0),
        KeyRow::Slot(1),
        KeyRow::Header(KeyGroup::Playback),
        KeyRow::Slot(7),
    ];
    assert_eq!(first_focusable(&rows), 1);
    assert_eq!(step_focus(&rows, 2, true), 4, "下移跨过标题");
    assert_eq!(step_focus(&rows, 4, false), 2, "上移跨过标题");
    assert_eq!(
        step_focus(&rows, 4, true),
        1,
        "末条下移回到首条，跳过首行标题"
    );
    assert_eq!(step_focus(&rows, 1, false), 4, "首条上移回到末条");

    for view in [KeyView::Host, KeyView::Fullscreen] {
        let rows = key_rows(view);
        let start = first_focusable(rows);
        let mut index = start;
        let mut steps = 0;
        loop {
            index = step_focus(rows, index, true);
            assert!(!rows[index].is_header(), "{view:?} 焦点落在标题上");
            steps += 1;
            if index == start {
                break;
            }
        }
        let entries = rows.iter().filter(|row| !row.is_header()).count();
        assert_eq!(steps, entries, "{view:?} 走一圈应经过每一条");
    }
}

/// 主程序列表里同名的两侧槽位靠作用域后缀区分；全屏页不加后缀。
#[test]
fn host_view_qualifies_one_sided_slot_names() {
    let config = Config::default();
    let lang = Language::Zh;
    let host_prev = row_text(KeyRow::Slot(7), &config, lang, KeyView::Host);
    let fs_prev = row_text(KeyRow::Slot(10), &config, lang, KeyView::Host);
    assert_ne!(host_prev, fs_prev);
    assert!(host_prev.contains("（主程序）"), "{host_prev}");
    assert!(fs_prev.contains("（全屏）"), "{fs_prev}");
    assert_eq!(
        row_text(KeyRow::Slot(10), &config, lang, KeyView::Fullscreen),
        format!("上一首: {}", config.keybind_fullscreen_prev)
    );
    assert!(
        !row_text(KeyRow::Slot(1), &config, lang, KeyView::Host).contains('（'),
        "两侧生效的槽位不加后缀"
    );
}

/// 每个槽位读写的是各自的配置字段（宏写错字段会让重绑改到别的键上）。
#[test]
fn slot_accessors_address_distinct_fields() {
    let mut config = Config::default();
    for (index, slot) in SLOTS.iter().enumerate() {
        *(slot.get_mut)(&mut config) = format!("F{}", index + 1);
    }
    for (index, slot) in SLOTS.iter().enumerate() {
        assert_eq!((slot.get)(&config), &format!("F{}", index + 1));
    }
}
