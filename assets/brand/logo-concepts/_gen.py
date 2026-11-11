#!/usr/bin/env python3
"""生成 miconlab 品牌标记（已选定的 B 方案 · 方波 M）：三套配色 + 多尺寸 PNG。

标记的正式用法是 assets/brand/logo/ 下的组合标；本目录保留标记单独一版，
用于需要纯图形（无字标）的场合，例如应用图标与 favicon。
"""
import os
import subprocess
from PIL import Image

BASE = os.path.dirname(os.path.abspath(__file__))
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

PRIMARY = {"bg": "#0B1220", "mark": "#22D3EE", "accent": "#38BDF8"}
REVERSE = {"bg": "#FFFFFF", "mark": "#0B1220", "accent": "#0EA5E9"}
MONO = {"bg": "#FFFFFF", "mark": "#000000", "accent": "#000000"}

SIZES = [16, 32, 128, 512]
KEY = "b-wave-m"
NAME = "方波 · M"
IDEA = "用一段方波连成 M，呼应示波器与串口时序，峰值处加实心节点。"


def tile(body, c):
    """统一外包一层圆角底板，保证 16px 下有稳定轮廓。"""
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
  <rect width="512" height="512" rx="112" fill="{c['bg']}"/>
{body.format(**c)}
</svg>
'''


BODY = '''  <path d="M96 360V208l64 96 64-96 64 96 64-96 64 96" fill="none"
        stroke="{mark}" stroke-width="64" stroke-linejoin="miter" stroke-linecap="butt"/>
  <g fill="{accent}">
    <rect x="64" y="176" width="64" height="64"/>
    <rect x="384" y="176" width="64" height="64"/>
  </g>'''


def write_svg(variant, palette):
    path = os.path.join(BASE, KEY, f"logo-{KEY}-{variant}.svg")
    with open(path, "w") as f:
        f.write(tile(BODY, palette))
    return path


def render(svg, out_png, size=512):
    subprocess.run(
        [CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars",
         "--force-device-scale-factor=1", f"--window-size={size},{size}",
         f"--screenshot={out_png}", f"file://{svg}"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True,
    )
    return out_png


def main():
    d = os.path.join(BASE, KEY)
    os.makedirs(d, exist_ok=True)
    primary = write_svg("primary", PRIMARY)
    write_svg("reverse", REVERSE)
    write_svg("mono", MONO)

    big = os.path.join(d, "logo-512.png")
    render(primary, big)
    src = Image.open(big).convert("RGB")
    for s in SIZES:
        if s == 512:
            continue
        src.resize((s, s), Image.LANCZOS).save(os.path.join(d, f"logo-{s}.png"))

    # 反白与单色同样按 512 渲染再降采样，避免窗口裁切
    for variant in ("reverse", "mono"):
        svg = os.path.join(d, f"logo-{KEY}-{variant}.svg")
        tmp = os.path.join(d, f"_tmp-{variant}.png")
        render(svg, tmp)
        Image.open(tmp).convert("RGB").resize((128, 128), Image.LANCZOS).save(
            os.path.join(d, f"logo-{variant}-128.png"))
        os.remove(tmp)
    print(f"done: {KEY}（{NAME}）— {IDEA}")


if __name__ == "__main__":
    main()
