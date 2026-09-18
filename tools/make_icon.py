"""生成 assets/app.ico + app.png + icon_preview.png（构建期依赖 Pillow）。

设计：深蓝→亮蓝竖向渐变圆角方底 + 白色粗「S」+ 右下角浅色状态点（网络/系统感）。
小尺寸（16/24）会加粗笔画保证可读。
"""
import os

from PIL import Image, ImageDraw

SIZES = [16, 24, 32, 48, 64, 128, 256]
# 品牌蓝：顶亮底深
C_TOP = (72, 140, 248)
C_BOT = (32, 86, 196)
C_S = (255, 255, 255)
C_DOT = (120, 220, 160)   # 右下「在线」绿点


def _lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def _rounded_mask(size: int, radius: int) -> Image.Image:
    m = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    return m


def _draw_s(d: ImageDraw.ImageDraw, s: int, stroke: int):
    """几何化 S：上右开口弧 + 下左开口弧 + 中间横连。"""
    # 用椭圆边框模拟圆弧笔画（比 arc 更可控）
    cy = s // 2
    # 上半环（右下开口）
    box_t = [int(s * 0.20), int(s * 0.14), int(s * 0.80), int(s * 0.56)]
    d.arc(box_t, start=15, end=250, fill=C_S, width=stroke)
    # 下半环（左上开口）
    box_b = [int(s * 0.20), int(s * 0.44), int(s * 0.80), int(s * 0.86)]
    d.arc(box_b, start=195, end=70, fill=C_S, width=stroke)
    # 中间连接斜杠
    y = int(cy - stroke * 0.15)
    d.line([(int(s * 0.28), y + stroke // 2), (int(s * 0.72), y - stroke // 2)],
           fill=C_S, width=stroke)


def _draw_one(size: int) -> Image.Image:
    ss = max(8, size * 4)  # 超采样
    # 渐变底
    base = Image.new("RGBA", (ss, ss), (0, 0, 0, 0))
    for y in range(ss):
        t = y / max(1, ss - 1)
        col = _lerp(C_TOP, C_BOT, t) + (255,)
        ImageDraw.Draw(base).line([(0, y), (ss, y)], fill=col)

    mask = _rounded_mask(ss, radius=int(ss * 0.22))
    out = Image.new("RGBA", (ss, ss), (0, 0, 0, 0))
    out.paste(base, (0, 0), mask)

    d = ImageDraw.Draw(out)
    # 顶部内高光
    hl = Image.new("RGBA", (ss, ss), (0, 0, 0, 0))
    hd = ImageDraw.Draw(hl)
    hd.ellipse([int(ss * 0.18), int(ss * -0.15), int(ss * 0.82), int(ss * 0.55)],
               fill=(255, 255, 255, 50))
    hl.putalpha(Image.composite(hl.split()[3], Image.new("L", (ss, ss), 0), mask))
    out = Image.alpha_composite(out, hl)
    d = ImageDraw.Draw(out)

    # S：小尺寸更粗
    stroke = max(int(ss * 0.11), 3) if size <= 32 else max(int(ss * 0.085), 4)
    _draw_s(d, ss, stroke)

    # 右下状态点
    r = int(ss * 0.09)
    cx, cy = int(ss * 0.78), int(ss * 0.78)
    d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=C_DOT + (255,))
    # 点上的小高光
    hr = max(1, r // 3)
    d.ellipse([cx - hr, cy - r - hr // 2, cx + hr, cy - r // 3], fill=(255, 255, 255, 90))

    return out.resize((size, size), Image.LANCZOS)


def _preview(sizes, out_path: str):
    pad = 16
    cell = 256 + pad * 2
    canvas = Image.new("RGBA", (cell * len(sizes), cell), (243, 245, 249, 255))
    d = ImageDraw.Draw(canvas)
    for i, n in enumerate(sizes):
        icon = _draw_one(n)
        # 缩放显示：小图标也放大到 128 便于预览
        show = 128 if n <= 64 else 256
        icon = icon.resize((show, show), Image.LANCZOS)
        x = i * cell + (cell - show) // 2
        y = (cell - show) // 2
        canvas.paste(icon, (x, y), icon)
        d.text((i * cell + 8, cell - 22), f"{n}px", fill=(90, 100, 115, 255))
    canvas.save(out_path)


def main():
    root = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
    out_dir = os.path.join(root, "assets")
    os.makedirs(out_dir, exist_ok=True)
    imgs = [_draw_one(n) for n in SIZES]
    out = os.path.join(out_dir, "app.ico")
    # ICO：先大后小顺序，Pillow 用 append_images 补其余尺寸
    imgs[-1].save(out, format="ICO",
                  sizes=[(x, x) for x in SIZES],
                  append_images=imgs[:-1])
    imgs[-1].save(os.path.join(out_dir, "app.png"))
    _preview(SIZES, os.path.join(out_dir, "icon_preview.png"))
    print("icon ->", out)
    print("png   ->", os.path.join(out_dir, "app.png"))
    print("preview ->", os.path.join(out_dir, "icon_preview.png"))


if __name__ == "__main__":
    main()
