"""像素比对两棵 `tools/ui_preview.py` 截图目录，用于「行为不变的改动」验收。

为什么需要它：ui_preview.py 的说明里写着截图用于改动前后对比，但此前只能靠肉眼看图，
表格列宽/隔行底色这类细微回归很容易看漏。纯 UI 收敛（如 theme.make_table 收编 16 张
页面表格）应当要求**逐像素零差异**，而不是「看起来一样」。

用法：
    python tools/diff_preview.py <改前目录> <改后目录>
输出每行 `SAME` / `DIFF`（附不同像素数）/ `SIZE-DIFF` / `ONLY-A` / `ONLY-B`；
全部一致时退出码 0，否则 1（可直接接进 CI 或脚本判断）。

注意：两棵目录必须用**同一版 ui_preview.py**渲染——脚手架的假 mock 一变
（例如补上 `netcheck.run_all` 的 `should_stop` 形参），诊断页填充内容就会改变
窗口宽度，比对结果与产品代码无关。
"""
import os
import sys

from PySide6.QtGui import QImage


def _load(path):
    img = QImage(path)
    if img.isNull():
        raise SystemExit(f"无法读取截图：{path}")
    return img.convertToFormat(QImage.Format_ARGB32)


def _diff(a, b):
    """返回不同像素数；尺寸不同返回 None（不可比）。"""
    if a.size() != b.size():
        return None
    bad = 0
    for y in range(a.height()):
        sa = bytes(a.constScanLine(y))
        sb = bytes(b.constScanLine(y))
        if sa == sb:                       # 整行相同走快路径，逐像素只在差异行展开
            continue
        for x in range(a.width()):
            k = x * 4
            if sa[k:k + 4] != sb[k:k + 4]:
                bad += 1
    return bad


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    dir_a, dir_b = sys.argv[1], sys.argv[2]
    fa = {f for f in os.listdir(dir_a) if f.endswith(".png")}
    fb = {f for f in os.listdir(dir_b) if f.endswith(".png")}
    failed = False
    for name in sorted(fa & fb):
        a = _load(os.path.join(dir_a, name))
        b = _load(os.path.join(dir_b, name))
        bad = _diff(a, b)
        if bad is None:
            print(f"SIZE-DIFF {name}  {a.width()}x{a.height()} vs "
                  f"{b.width()}x{b.height()}")
            failed = True
        elif bad:
            print(f"DIFF      {name}  不同像素 {bad}/{a.width() * a.height()}")
            failed = True
        else:
            print(f"SAME      {name}")
    for name in sorted(fa - fb):
        print(f"ONLY-A    {name}")
        failed = True
    for name in sorted(fb - fa):
        print(f"ONLY-B    {name}")
        failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
