"""生成 assets/app.ico（构建期依赖 Pillow，仅此一处使用）。"""
import os

from PIL import Image, ImageDraw

SIZES = [16, 24, 32, 48, 64, 128, 256]


def main():
    os.makedirs(os.path.join(os.path.dirname(__file__), "..", "assets"), exist_ok=True)
    imgs = []
    for size in SIZES:
        s = size * 4  # 超采样抗锯齿
        img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
        d = ImageDraw.Draw(img)
        m = s // 16
        d.ellipse([m, m, s - m, s - m], fill=(53, 118, 224, 255))
        # 高光
        d.ellipse([s // 5, s // 6, s // 5 + s // 3, s // 6 + s // 4], fill=(255, 255, 255, 70))
        # 白色扳手形 → 简化为 "N"
        w = max(4, s // 8)
        x0, y0, x1 = s * 0.32, s * 0.28, s * 0.68
        d.line([(x0, s * 0.72), (x0, y0), (x1, s * 0.72), (x1, y0)], fill="white", width=w, joint="curve")
        img = img.resize((size, size), Image.LANCZOS)
        imgs.append(img)
    out = os.path.join(os.path.dirname(__file__), "..", "assets", "app.ico")
    imgs[0].save(out, format="ICO", sizes=[(x, x) for x in SIZES], append_images=imgs[1:])
    print("icon ->", os.path.abspath(out))


if __name__ == "__main__":
    main()
