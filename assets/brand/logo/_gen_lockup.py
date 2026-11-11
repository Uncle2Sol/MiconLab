#!/usr/bin/env python3
"""生成 MiconLab 组合标（横版/竖版 × 主色/反白/单色）与多尺寸预览。

字标全部用 stroke 路径绘制（不依赖系统字体），与 B 方案方波标记同构：等距、直角、无衬线。
"""
import os
import subprocess
from PIL import Image, ImageDraw

BASE = os.path.dirname(os.path.abspath(__file__))
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

PRIMARY = {"bg": "#0B1220", "mark": "#22D3EE", "accent": "#38BDF8"}
REVERSE = {"bg": "#FFFFFF", "mark": "#0B1220", "accent": "#0EA5E9"}
MONO = {"bg": "#FFFFFF", "mark": "#000000", "accent": "#000000"}

# 每个字形：宽度 + 路径（基线 y=100，大写高 0–100，小写 x 高 32–100）
GLYPHS = [
    ("M", 86, "M6 100V6l37 56L80 6v94"),
    ("i", 14, "M7 36V100"),
    ("c", 64, "M52.5 52A26 26 0 1 0 52.5 84"),
    ("o", 64, "M58 68a26 26 0 1 0-52 0a26 26 0 1 0 52 0"),
    ("n", 56, "M6 100V54a22 22 0 0 1 44 0v46"),
    ("L", 70, "M6 6v94h58"),
    ("a", 64, "M58 68a26 26 0 1 0-52 0a26 26 0 1 0 52 0M58 42v58"),
    ("b", 64, "M6 0v100M6 68a26 26 0 1 1 52 0a26 26 0 1 1-52 0"),
]
TRACK = 16
# 四周安全边距（设计坐标，cap=100）：保证描边不贴 viewBox 边线，避免看起来像被裁切
PAD = 6


def wordmark(x, y, scale, color, stroke):
    """返回字标 <g>：每个字母一条 stroke 路径，i 的点为实心方块。"""
    parts = [f'<g transform="translate({x} {y}) scale({scale})" '
             f'fill="none" stroke="{color}" stroke-width="{stroke}" '
             f'stroke-linecap="square" stroke-linejoin="miter">']
    cursor = 0.0
    for ch, w, d in GLYPHS:
        parts.append(f'<g transform="translate({cursor:.1f} 0)"><path d="{d}"/>')
        if ch == "i":
            parts.append(f'<rect x="1" y="8" width="12" height="12" fill="{color}" stroke="none"/>')
        parts.append("</g>")
        cursor += w + TRACK
    parts.append("</g>")
    return "".join(parts), cursor - TRACK


# 标记在 512 设计坐标里的实际着墨范围（含 64 描边外扩），用于按视觉高度对齐字标
MARK_BOX = (64.0, 176.0, 416.0, 248.0)


def mark(x, y, drawn_h, c):
    """B 方案标记：方波 M + 两个峰值节点，按着墨高度 drawn_h 缩放并对齐到 (x, y)。"""
    s = drawn_h / MARK_BOX[3]
    tx, ty = x - MARK_BOX[0] * s, y - MARK_BOX[1] * s
    return (f'<g transform="translate({tx:.3f} {ty:.3f}) scale({s:.6f})">'
            f'<path d="M96 360V208l64 96 64-96 64 96 64-96 64 96" fill="none" '
            f'stroke="{c["mark"]}" stroke-width="64" stroke-linejoin="miter" stroke-linecap="butt"/>'
            f'<rect x="64" y="176" width="64" height="64" fill="{c["accent"]}"/>'
            f'<rect x="384" y="176" width="64" height="64" fill="{c["accent"]}"/>'
            "</g>")


def wm_width():
    """字标着墨宽度（cap=100 时）"""
    return sum(w for _, w, _ in GLYPHS) + TRACK * (len(GLYPHS) - 1)


def horizontal(c, bg=None, note="", pad=PAD):
    """横版：标记在左、字标在右；标记着墨高度略高于 cap 高，做光学配重。

    bg 非空时在最底层加一块同尺寸深色底板，用于任意主题页面上的稳定可读；
    pad 可放大留白，让底板成为一张完整的“卡片”。
    """
    cap, mark_h, gap = 100, 108, 44
    mark_w = mark_h * MARK_BOX[2] / MARK_BOX[3]
    content_w = mark_w + gap + wm_width()
    width = content_w + 2 * pad
    height = mark_h + 2 * pad
    plate = (
        f'  <rect width="{width:.1f}" height="{height:.1f}" rx="24" fill="{bg}"/>\n'
        if bg else ""
    )
    wm, _ = wordmark(pad + mark_w + gap, pad + (mark_h - cap) / 2, 1.0, c["mark"], 12)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width:.1f} {height:.1f}" '
            f'width="{width:.1f}" height="{height:.1f}">\n'
            f'{note}{plate}  {mark(pad, pad, mark_h, c)}\n  {wm}\n</svg>\n')


def vertical(c, note=""):
    """竖版：标记居中在上、字标居中在下；标记放大到足以压住字标宽度。"""
    cap, mark_h, gap = 100, 150, 44
    wm_w = wm_width()
    mark_w = mark_h * MARK_BOX[2] / MARK_BOX[3]
    width = max(mark_w, wm_w) + 2 * PAD
    height = mark_h + gap + cap + 2 * PAD
    wm, _ = wordmark((width - wm_w) / 2, PAD + mark_h + gap, 1.0, c["mark"], 12)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width:.1f} {height:.1f}" '
            f'width="{width:.1f}" height="{height:.1f}">\n'
            f'{note}  {mark((width - mark_w) / 2, PAD, mark_h, c)}\n  {wm}\n</svg>\n')


def render(svg, out, w, h):
    """按目标像素尺寸重写 SVG 的 width/height 再截图，避免被窗口裁切。"""
    import re
    src = open(svg).read()
    sized = re.sub(r'width="[^"]+" height="[^"]+"', f'width="{w:.0f}" height="{h:.0f}"', src, count=1)
    tmp = svg.replace(".svg", ".tmp.svg")
    with open(tmp, "w") as f:
        f.write(sized)
    subprocess.run(
        [CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars",
         "--default-background-color=00000000", "--force-device-scale-factor=1",
         f"--window-size={int(w)},{int(h)}", f"--screenshot={out}", f"file://{tmp}"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    os.remove(tmp)


def main():
    sizes = [16, 32, 128, 512]
    variants = {"primary": PRIMARY, "reverse": REVERSE, "mono": MONO}
    made = {}
    for layout, fn in (("horizontal", horizontal), ("vertical", vertical)):
        for vname, pal in variants.items():
            svg = os.path.join(BASE, f"miconlab-lockup-{layout}-{vname}.svg")
            # 每个文件都写明来源：主版为唯一源，其余标注派生关系（同几何，仅换配色或版式）
            if vname == "primary" and layout == "horizontal":
                note = ("<!-- MiconLab 主 logo 源文件（横版 · 主色）。"
                        "其余同族文件均由本文件派生：几何完全相同，仅替换配色或版式。 -->\n")
            elif vname == "primary":
                note = ("<!-- 派生自 miconlab-lockup-horizontal-primary.svg：同几何，仅改为竖版布局。 -->\n")
            elif layout == "horizontal":
                note = (f"<!-- 派生自 miconlab-lockup-horizontal-primary.svg：同几何，仅替换为"
                        f"{'浅底反白' if vname == 'reverse' else '纯黑单色'}配色。 -->\n")
            else:
                note = ("<!-- 派生自 miconlab-lockup-horizontal-primary.svg：同几何，仅改为竖版布局"
                        "并替换配色。 -->\n")
            with open(svg, "w") as f:
                f.write(fn(pal, note=note))
            # 512 高渲染，再按高度降采样
            big = os.path.join(BASE, f".tmp-{layout}-{vname}.png")
            # 取 SVG 的宽高比
            head = open(svg).read()
            vb = head.split('viewBox="0 0 ')[1].split('"')[0].split()
            vw, vh = float(vb[0]), float(vb[1])
            scale = 512 / vh
            render(svg, big, vw * scale, 512)
            src = Image.open(big).convert("RGBA")
            for s in sizes:
                h = s
                w = round(vw / vh * h)
                img = src.resize((w, h), Image.LANCZOS)
                bg = Image.new("RGBA", img.size, pal["bg"])
                bg.alpha_composite(img)
                bg.convert("RGB").save(
                    os.path.join(BASE, f"miconlab-lockup-{layout}-{vname}-{s}.png"))
            os.remove(big)
            made[(layout, vname)] = os.path.join(
                BASE, f"miconlab-lockup-{layout}-{vname}-128.png")

    # README 专用：主色 + 深底底板（单张即可在浅色/深色主题下稳定可读，不依赖 prefers-color-scheme）
    on_dark = os.path.join(BASE, "miconlab-lockup-horizontal-primary-on-dark.svg")
    with open(on_dark, "w") as f:
        f.write(horizontal(
            PRIMARY, bg=PRIMARY["bg"], pad=30,
            note=("<!-- 派生自 miconlab-lockup-horizontal-primary.svg：同几何，"
                  "仅叠加 #0B1220 深底圆角底板与 30 单位留白，供 README 等任意主题页面使用。 -->\n")))

    # 对比图：三行配色 × 两列表（横版 / 竖版）
    cell_w, cell_h, pad, label_h = 560, 300, 18, 0
    W = 2 * cell_w + 3 * pad
    H = 3 * (cell_h + pad) + pad + 24
    canvas = Image.new("RGB", (W, H), "#F2F4F7")
    draw = ImageDraw.Draw(canvas)
    for row, vname in enumerate(["primary", "reverse", "mono"]):
        for col, layout in enumerate(["horizontal", "vertical"]):
            x = pad + col * (cell_w + pad)
            y = pad + row * (cell_h + pad)
            canvas.paste(Image.new("RGB", (cell_w, cell_h), variants[vname]["bg"]), (x, y))
            im = Image.open(made[(layout, vname)]).convert("RGB")
            ratio = min((cell_w - 40) / im.width, (cell_h - 40) / im.height)
            im = im.resize((int(im.width * ratio), int(im.height * ratio)), Image.LANCZOS)
            canvas.paste(im, (x + (cell_w - im.width) // 2, y + (cell_h - im.height) // 2))
    draw.text((pad, H - 20), "rows: primary / reverse / mono    cols: horizontal / vertical",
              fill="#111827")
    canvas.save(os.path.join(BASE, "lockup-overview.png"))
    print("lockups done")


if __name__ == "__main__":
    main()
