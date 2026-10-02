/**
 * 场景模式纯逻辑测试。
 *
 * 重点是**如实汇报失败步骤**：只说"已进入游戏模式"会让用户以为全部生效，
 * 而"暂停更新需要管理员"这类步骤其实没做。另：暂停更新的文案不得暗示到期自动恢复。
 */
import { describe, expect, it } from "vitest";

import {
  adminNote,
  enterConfirmText,
  failedSteps,
  isEntering,
  modeLabel,
  outcomeLevel,
  outcomeSummary,
  updatePolicyNote,
  type ModeOutcome,
  type ModeStep,
  type ModesInfo,
} from "../src/modes";

const step = (name: string, ok: boolean, detail = ""): ModeStep => ({ name, ok, detail });

const outcome = (steps: ModeStep[], mode = "游戏模式"): ModeOutcome => ({
  mode,
  steps,
  ok_count: steps.filter((s) => s.ok).length,
  fail_count: steps.filter((s) => !s.ok).length,
});

const info = (over: Partial<ModesInfo> = {}): ModesInfo => ({
  common_apps: [{ name: "记事本", target: "notepad.exe" }],
  office_default_apps: ["记事本"],
  whitelist: ["OneDrive.exe"],
  is_admin: true,
  notes: {},
  ...over,
});

describe("modeLabel / isEntering", () => {
  it("四个模式的文案", () => {
    expect(modeLabel("office")).toBe("进入办公模式");
    expect(modeLabel("office_exit")).toBe("退出办公模式");
    expect(modeLabel("game")).toBe("进入游戏模式");
    expect(modeLabel("game_exit")).toBe("退出游戏模式");
  });

  it("只有「进入」是改动型（退出是恢复，不需二次确认）", () => {
    expect(isEntering("office")).toBe(true);
    expect(isEntering("game")).toBe(true);
    expect(isEntering("office_exit")).toBe(false);
    expect(isEntering("game_exit")).toBe(false);
  });
});

describe("outcomeLevel", () => {
  it("全成 success，部分失败 warning，全败 error", () => {
    expect(outcomeLevel(outcome([step("a", true), step("b", true)]))).toBe("success");
    expect(outcomeLevel(outcome([step("a", true), step("b", false)]))).toBe("warning");
    expect(outcomeLevel(outcome([step("a", false), step("b", false)]))).toBe("error");
  });

  it("null 时为 success（不显示告警）", () => {
    expect(outcomeLevel(null)).toBe("success");
  });
});

describe("outcomeSummary 诚实性", () => {
  it("全成时报总步数", () => {
    const s = outcomeSummary(outcome([step("a", true), step("b", true)]));
    expect(s).toBe("游戏模式：2 步全部完成");
  });

  it("有失败时必须点名失败步骤", () => {
    // 只说"已进入游戏模式"会让用户以为暂停更新也生效了
    const s = outcomeSummary(
      outcome([step("通知/勿扰", true), step("暂停 Windows 更新", false, "需要管理员权限")]),
    );
    expect(s).toContain("1/2");
    expect(s).toContain("暂停 Windows 更新");
  });

  it("未执行时明确说明", () => {
    expect(outcomeSummary(null)).toBe("尚未执行模式");
  });
});

describe("failedSteps", () => {
  it("只返回失败步骤", () => {
    const o = outcome([step("a", true), step("b", false), step("c", false)]);
    const f = failedSteps(o);
    expect(f.map((s) => s.name)).toEqual(["b", "c"]);
  });

  it("无失败时为空；null 时为空", () => {
    expect(failedSteps(outcome([step("a", true)]))).toEqual([]);
    expect(failedSteps(null)).toEqual([]);
  });
});

describe("enterConfirmText", () => {
  it("办公模式列出将做的三件事", () => {
    const t = enterConfirmText("office", { openApps: true, clearBg: true });
    expect(t).toContain("通知");
    expect(t).toContain("静音");
    expect(t).toContain("备份");
    expect(t).toContain("退出");
  });

  it("办公模式勾掉开软件时如实说明不打开", () => {
    const t = enterConfirmText("office", { openApps: false, clearBg: true });
    expect(t).toContain("不自动打开");
    expect(t).not.toContain("记事本");
  });

  it("游戏模式说明更新策略是持久的，且白名单不碰系统进程", () => {
    const t = enterConfirmText("game", { openApps: true, clearBg: true });
    expect(t).toContain("持久");
    expect(t).toContain("不碰系统进程");
    expect(t).toContain("高性能");
  });

  it("游戏模式勾掉清后台时如实说明", () => {
    const t = enterConfirmText("game", { openApps: true, clearBg: false });
    expect(t).toContain("不清理后台");
  });
});

describe("updatePolicyNote", () => {
  it("后端给了说明就用它", () => {
    expect(updatePolicyNote(info({ notes: { update_policy: "自定义说明" } }))).toBe("自定义说明");
  });

  it("兜底文案不得暗示到期自动恢复", () => {
    const t = updatePolicyNote(null);
    expect(t).toContain("持久");
    expect(t).not.toContain("7 天");
    expect(t).not.toContain("自动恢复");
  });
});

describe("adminNote", () => {
  it("非管理员时给出提示", () => {
    expect(adminNote(info({ is_admin: false }))).toContain("管理员");
  });

  it("管理员时为空（不打扰）", () => {
    expect(adminNote(info({ is_admin: true }))).toBe("");
    expect(adminNote(null)).toBe("");
  });
});
