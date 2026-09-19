"""入口：无参 → GUI；子命令 → CLI。"""
import sys


def run():
    argv = sys.argv[1:]
    if argv and argv[0] in ("diagnose", "repair", "help", "--help", "-h"):
        if argv[0] in ("help", "--help", "-h"):
            from .cli import main
            sys.exit(main([]))
        from .cli import main
        sys.exit(main(argv))
    # 未知子命令/未知选项明确报错（曾静默弹整个 GUI，掩盖拼写错误如
    # "dianose" / "--minimizd"——后者表现为行为异常但退出码 0）
    known_options = {"--minimized"}
    if argv and argv[0] not in ("diagnose", "repair", "help", "--help", "-h"):
        if not argv[0].startswith("-"):
            print(f"未知命令: {argv[0]}\n可用: diagnose / repair / help；无参数启动图形界面。")
            sys.exit(2)
        if argv[0] not in known_options and "--minimized" not in argv:
            print(f"未知选项: {argv[0]}\n可用选项: --minimized；无参数启动图形界面。")
            sys.exit(2)
    # GUI 模式（--minimized 透传给 argv，由 app.py 识别）
    try:
        from .ui.app import run_gui
    except ImportError as e:
        print(f"GUI 依赖缺失（PySide6 未安装？）: {e}\n可使用命令行: python -m syskit diagnose")
        sys.exit(1)
    run_gui()


if __name__ == "__main__":
    run()
