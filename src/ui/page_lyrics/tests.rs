use super::*;

fn area(x: u16, y: u16, width: u16, height: u16) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// 默认位置 (1,1) 必须与历史行为一致：贴内容区右下角。
#[test]
fn default_position_is_bottom_right() {
    let content = area(0, 0, 120, 30);
    let panel = overlay_panel_area(content, (1.0, 1.0));

    assert_eq!(panel.x + panel.width, content.x + content.width);
    assert_eq!(panel.y + panel.height, content.y + content.height);
    assert_eq!(panel.height, PAGE_LYRICS_PANEL_HEIGHT);
}

#[test]
fn position_is_normalized_inside_the_content_area() {
    let content = area(3, 2, 100, 20);
    let size = panel_size(content).expect("放得下");

    for pos in [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)] {
        let panel = overlay_panel_area(content, pos);
        assert!(panel.x >= content.x && panel.y >= content.y);
        assert!(panel.x + panel.width <= content.x + content.width);
        assert!(panel.y + panel.height <= content.y + content.height);
        assert_eq!((panel.width, panel.height), size);
    }
}

/// 内容区放不下时返回空矩形（调用方据此跳过绘制与命中登记）。
#[test]
fn no_panel_when_the_area_is_too_small() {
    assert_eq!(
        overlay_panel_area(area(0, 0, 17, 30), (1.0, 1.0)),
        Rect::default()
    );
    assert_eq!(
        overlay_panel_area(area(0, 0, 80, 3), (1.0, 1.0)),
        Rect::default()
    );
}

/// 拖拽保持抓取偏移：光标下的那一点始终贴着浮窗同一处。
#[test]
fn drag_keeps_the_grab_offset() {
    let content = area(0, 0, 100, 20);
    let panel = overlay_panel_area(content, (1.0, 1.0));
    let grab = (5u16, 1u16);

    // 把光标放到内容区左上角 + grab：浮窗应回到 (0,0)
    let pos = pos_after_drag(content, panel, content.x + grab.0, content.y + grab.1, grab);
    assert_eq!(pos, (0.0, 0.0));

    // 越界拖动被钳制在内容区内
    let pos = pos_after_drag(content, panel, content.x + 500, content.y + 500, grab);
    assert_eq!(pos, (1.0, 1.0));
}

/// 吸附到最近的**边**（另一轴保持自由），不是吸到角。
#[test]
fn snap_pulls_to_the_nearest_edge() {
    let content = area(0, 0, 100, 20);
    let size = panel_size(content).expect("放得下");

    // 贴左边、纵向在中间：只吸 x，y 保持
    let left = Rect {
        x: content.x + 1,
        y: content.y + 6,
        ..panel_rect_at(content, size, (0.0, 0.5))
    };
    assert_eq!(snap_pos(content, left, (0.3, 0.5)), (0.0, 0.5));

    // 贴下边、横向居中：只吸 y，x 保持
    let bottom = Rect {
        x: content.x + 40,
        y: content.y + 20 - size.1,
        ..panel_rect_at(content, size, (0.5, 1.0))
    };
    assert_eq!(snap_pos(content, bottom, (0.5, 0.3)), (0.5, 1.0));

    // 本来就在右下角：两轴都吸
    let corner = panel_rect_at(content, size, (1.0, 1.0));
    assert_eq!(snap_pos(content, corner, (0.4, 0.4)), (1.0, 1.0));
}
