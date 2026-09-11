"""构建第 4/5 步：把 dist/SysKit.exe 压成 SysKit-<版本>-portable.zip。

`--print-version` 只打印 config.VERSION（供 build.bat 捕获传给安装器脚本）。
"""
import os
import re
import sys
import zipfile

SRC = os.path.join("dist", "SysKit.exe")


def read_version() -> str:
    cfg = open(os.path.join("syskit", "config.py"), encoding="utf-8").read()
    m = re.search(r'VERSION\s*=\s*"([^"]+)"', cfg)
    return m.group(1) if m else "0.0.0"


def main():
    if "--print-version" in sys.argv:
        print(read_version())
        return
    if not os.path.exists(SRC):
        raise SystemExit(f"未找到 {SRC}，请先完成 pyinstaller 打包")
    out = os.path.join("dist", f"SysKit-{read_version()}-portable.zip")
    if os.path.exists(out):
        os.remove(out)
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        z.write(SRC, "SysKit.exe")
    print("zip ->", out)


if __name__ == "__main__":
    main()
