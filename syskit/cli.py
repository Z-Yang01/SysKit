"""命令行模式：diagnose / repair。"""
import argparse
import sys

from . import admin, logger, netcheck, repairs
from .logger import LOGGER


def cmd_diagnose(quick: bool = False) -> int:
    log = LOGGER
    if not admin.is_admin():
        log.warning("!! 未以管理员运行：部分检查项将受限（诊断结果仍可用于定位主要问题）\n")
    log.info("正在采集网络状态（约 5-15 秒）...")
    results = netcheck.run_all(progress=lambda r: log.info(f"  {r.name} ... {r.status}"),
                               quick=quick)
    log.info("")
    log.info("=" * 62)
    log.info("诊断报告  SysKit")
    log.info("=" * 62)
    for r in results:
        log.info(r.line())
    base, text = netcheck.verdict(results)
    log.info("-" * 62)
    log.info(f"综合判定: {text}")
    fails = [r for r in results if r.fix_ids and r.status in (netcheck.FAIL, netcheck.WARN)]
    if fails:
        log.info("\n建议修复项:")
        for r in fails:
            for fid in dict.fromkeys(r.fix_ids):
                act = repairs.get_action(fid)
                if act:
                    log.info(f"  [{fid}] {act.title} —— {act.brief}")
    return 0


def cmd_repair(args) -> int:
    log = LOGGER
    if not admin.is_admin():
        log.error("修复操作需要管理员权限。请以管理员身份运行。")
        return 2
    # 组合定义复用 repairs.COMBOS（与 UI 同源，避免两处定义悄悄分叉）
    ids = (list(repairs.COMBOS[args.combo][1]) if args.combo in repairs.COMBOS
           else ([args.combo] if repairs.get_action(args.combo) else None))
    if not ids:
        log.error(f"未知修复项/组合: {args.combo}；可用: {', '.join(repairs.COMBOS)} 或单项 {', '.join(repairs.ACTION_IDS)}")
        return 2
    for fid in ids:
        act = repairs.get_action(fid)
        if not act:
            continue
        if not args.yes:
            log.info(f"\n[{fid}] {act.title}")
            log.info(f"  说明: {act.description}")
            log.info(f"  还原: {act.restore_hint}")
            ans = input(f"  执行? (y/N) ").strip().lower()
            if ans != "y":
                log.info("  已跳过")
                continue
        log.info(f"\n>>> 执行 [{fid}] {act.title} ...")
        result = act.execute()
        log.info(f"<<< {'成功' if result.success else '失败'}: {result.message}")
        if result.need_reboot:
            log.info("*** 此修复需要重启电脑后生效 ***")
    return 0


def main(argv=None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    p = argparse.ArgumentParser(prog="syskit", description="SysKit 系统工具箱")
    sub = p.add_subparsers(dest="cmd")
    d = sub.add_parser("diagnose", help="全量诊断")
    d.add_argument("--quick", action="store_true", help="跳过慢速项")
    r = sub.add_parser("repair", help="执行修复: fast|full|stack 或单项 id")
    r.add_argument("combo", nargs="?", default="fast")
    r.add_argument("-y", "--yes", action="store_true", help="跳过确认")
    args = p.parse_args(argv)
    logger.setup(verbose_console=True)
    if args.cmd == "diagnose":
        return cmd_diagnose(quick=args.quick)
    if args.cmd == "repair":
        return cmd_repair(args)
    p.print_help()
    return 0
