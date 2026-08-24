#!/usr/bin/env bash
# 预烘焙 about 彩蛋的形象动画帧。
#
# 运行时不解码图片、不调用 chafa —— 全部在这里离线完成，产物是一份
# 纯数据的 Rust 源码。仅在更换形象或调整动画参数时需要重跑：
#
#   ./tools/gen_mascot_frames.sh
#
# 依赖：chafa（命令行）、python3。二者都不参与 cargo build。
set -euo pipefail

cd "$(dirname "$0")/.."

SRC="assets/mascot_src.png"
OUT="src/render/mascot_frames.rs"

if [[ ! -f "$SRC" ]]; then
  echo "缺少源图 $SRC" >&2
  echo "可由原始形象生成：magick <形象>.webp -resize 128x128 -background none $SRC" >&2
  exit 1
fi

if ! command -v chafa >/dev/null 2>&1; then
  echo "需要 chafa 命令行工具" >&2
  exit 1
fi

python3 - "$SRC" "$OUT" <<'PY'
import math
import subprocess
import sys

src, out = sys.argv[1], sys.argv[2]

# 形象显示区域（字符），与参考实现一致。
BASE_W, BASE_H = 20, 10
FRAMES = 18

# 果冻压扁 -> 回弹 -> 衰减震荡的 squash & stretch 曲线，取自参考实现。
KEYS = [
    # (time, horizontal, vertical)
    (0.00, 1.00, 1.00),
    (0.16, 1.20, 0.62),
    (0.32, 0.82, 1.20),
    (0.52, 1.10, 0.88),
    (0.72, 0.94, 1.07),
    (0.88, 1.03, 0.97),
    (1.00, 1.00, 1.00),
]


def jelly_transform(t):
    t = min(1.0, max(0.0, t))
    i = 0
    while i < len(KEYS) - 1 and t > KEYS[i + 1][0]:
        i += 1
    t0, sx0, sy0 = KEYS[i]
    t1, sx1, sy1 = KEYS[i + 1]
    local = 0.0 if abs(t1 - t0) < 1e-12 else (t - t0) / (t1 - t0)
    eased = (1.0 - math.cos(local * math.pi)) * 0.5
    return sx0 + (sx1 - sx0) * eased, sy0 + (sy1 - sy0) * eased


def render(width, height):
    # 参数与参考实现的 render_with_chafa 保持一致：--stretch 负责压扁/拉伸
    # （而非裁切），--symbols=block 把字符集限制在普遍支持的块元素上。
    cmd = [
        "chafa",
        "--format=symbols",
        "--colors=256",
        "--symbols=block",
        "--fill=block",
        "--dither=none",
        "--bg=#1e1e2e",
        f"--size={width}x{height}",
        "--stretch",
        "--relative=off",
        "--preprocess=off",
        "--probe=off",
        "--animate=off",
        src,
    ]
    return subprocess.run(cmd, check=True, capture_output=True).stdout.decode("utf-8")


def apply_sgr(param, state):
    params = param.split(";")
    if not params or params[0] == "" or params[0] == "0":
        state["fg"] = None
        state["bg"] = None
        state["reverse"] = False
        return

    i = 0
    while i < len(params):
        p = params[i]
        if p == "7":
            state["reverse"] = True
        elif p == "27":
            state["reverse"] = False
        elif p == "39":
            state["fg"] = None
        elif p == "49":
            state["bg"] = None
        elif p in ("38", "48") and i + 2 < len(params) and params[i + 1] == "5":
            key = "fg" if p == "38" else "bg"
            state[key] = int(params[i + 2]) & 0xFF
            i += 3
            continue
        i += 1


def parse_line(raw, width):
    """把一行 ANSI 文本解析为 width 个 (char, fg, bg)。"""
    state = {"fg": None, "bg": None, "reverse": False}
    cells = []
    chars = iter(raw)
    for ch in chars:
        if ch == "\x1b":
            seq = ""
            for nxt in chars:
                seq += nxt
                if nxt.isalpha():
                    break
            # 只关心 SGR（CSI ... m）；光标显隐等序列直接忽略。
            if seq.startswith("[") and seq.endswith("m"):
                apply_sgr(seq[1:-1], state)
            continue

        fg, bg = state["fg"], state["bg"]
        if state["reverse"]:
            fg, bg = bg, fg
        cells.append((ch, fg, bg))

    # chafa 会把行补齐到目标宽度，但反色/复位的位置可能让尾部缺格。
    while len(cells) < width:
        cells.append((" ", None, None))
    return cells[:width]


def parse_frame(text, width, height):
    rows = []
    for raw in text.split("\n"):
        # 去掉纯控制序列行（如首行的光标隐藏、末行的显示）。
        cells = parse_line(raw, width)
        if all(c[0] == " " and c[1] is None and c[2] is None for c in cells) and not rows:
            # 首部的空行来自控制序列，跳过。
            continue
        rows.append(cells)
        if len(rows) == height:
            break
    while len(rows) < height:
        rows.append([(" ", None, None)] * width)
    return rows[:height]


# 256 色索引最多到 255，用 0xFFFF 表示“不着色”。
NO_COLOR = 0xFFFF


def color_code(v):
    return NO_COLOR if v is None else v


def escape_str(s):
    return s.replace("\\", "\\\\").replace('"', '\\"')


frames = []
for i in range(FRAMES):
    t = i / (FRAMES - 1)
    sx, sy = jelly_transform(t)
    w = max(2, min(0xFFFF, round(BASE_W * sx)))
    h = max(1, min(0xFFFF, round(BASE_H * sy)))
    rows = parse_frame(render(w, h), w, h)
    frames.append((w, h, rows))

total_cells = sum(w * h for w, h, _ in frames)

lines = []
lines.append("// 由 tools/gen_mascot_frames.sh 生成，请勿手工编辑。")
lines.append("//")
lines.append("// 形象动画帧在构建前离线烘焙：运行时既不解码图片也不调用 chafa，只查表。")
lines.append(
    f"// 源图 {src}，基准区域 {BASE_W}x{BASE_H} 字符，{FRAMES} 帧共 {total_cells} 个单元。"
)
lines.append("")
lines.append("/// 颜色槽位取此值时不着色，沿用底层背景（256 色索引只用到 0..=255）。")
lines.append(f"pub const NO_COLOR: u16 = 0x{NO_COLOR:04X};")
lines.append("")
lines.append("/// 一帧形象。")
lines.append("///")
lines.append("/// `chars` 按行优先存放 width * height 个字符，`fg` / `bg` 与之逐字符对应，")
lines.append("/// 保存 256 色索引或 [`NO_COLOR`]。分成三个数组是为了让生成的源码保持紧凑。")
lines.append("pub struct MascotFrame {")
lines.append("    pub width: u16,")
lines.append("    pub height: u16,")
lines.append("    pub chars: &'static str,")
lines.append("    pub fg: &'static [u16],")
lines.append("    pub bg: &'static [u16],")
lines.append("}")
lines.append("")
lines.append("/// 形象静止时的区域大小（字符），也是果冻动画的基准尺寸。")
lines.append(f"pub const BASE_WIDTH: u16 = {BASE_W};")
lines.append(f"pub const BASE_HEIGHT: u16 = {BASE_H};")
lines.append("")


def emit_color_array(name, values, width, count):
    lines.append(f"static {name}: [u16; {count}] = [")
    for start in range(0, count, width):
        row = values[start : start + width]
        lines.append("    " + " ".join(f"{v}," for v in row))
    lines.append("];")
    lines.append("")


for idx, (w, h, rows) in enumerate(frames):
    count = w * h
    chars = "".join(ch for row in rows for ch, _, _ in row)
    fg = [color_code(c[1]) for row in rows for c in row]
    bg = [color_code(c[2]) for row in rows for c in row]

    lines.append(f"static CHARS_{idx}: &str = concat!(")
    for start in range(0, count, w):
        lines.append('    "' + escape_str(chars[start : start + w]) + '",')
    lines.append(");")
    lines.append("")
    emit_color_array(f"FG_{idx}", fg, w, count)
    emit_color_array(f"BG_{idx}", bg, w, count)

lines.append("/// 果冻动画的全部帧，索引 0 同时用作静止帧。")
lines.append(f"pub static FRAMES: [MascotFrame; {FRAMES}] = [")
for idx, (w, h, _) in enumerate(frames):
    lines.append("    MascotFrame {")
    lines.append(f"        width: {w},")
    lines.append(f"        height: {h},")
    lines.append(f"        chars: CHARS_{idx},")
    lines.append(f"        fg: &FG_{idx},")
    lines.append(f"        bg: &BG_{idx},")
    lines.append("    },")
lines.append("];")
lines.append("")

with open(out, "w", encoding="utf-8") as f:
    f.write("\n".join(lines))

print(f"已写入 {out}：{FRAMES} 帧，共 {total_cells} 个单元")
PY

if command -v rustfmt >/dev/null 2>&1; then
  rustfmt --edition 2024 "$OUT"
fi
