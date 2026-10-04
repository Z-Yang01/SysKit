/**
 * 磁盘空间与垃圾清理纯逻辑测试。
 *
 * 重点是清理的**安全与诚实**：不可逆操作要被校验拦住，结果必须如实汇报跳过项。
 */
import { describe, expect, it } from "vitest";

import {
  allTargetKeys,
  cleanConfirmText,
  cleanResultSummary,
  diskSummary,
  driveLabel,
  hasSkipped,
  humanSize,
  meaningfulKeys,
  needsSpaceWarning,
  selectedBytes,
  targetResultText,
  toggleKey,
  usageColor,
  validateClean,
  type CleanRunResult,
  type CleanTargetInfo,
  type DriveRow,
} from "../src/disk";

const drive = (over: Partial<DriveRow> = {}): DriveRow => ({
  letter: "C:",
  label: "System",
  fs: "NTFS",
  total: 1000,
  used: 900,
  free_total: 100,
  free_to_caller: 100,
  used_percent: 90,
  removable: false,
  token: "err",
  ...over,
});

const target = (over: Partial<CleanTargetInfo> = {}): CleanTargetInfo => ({
  key: "user_temp",
  title: "用户临时文件",
  root: "C:\\Users\\x\\AppData\\Local\\Temp",
  bytes: 1024,
  human: "1.0 KB",
  ...over,
});

const runResult = (over: Partial<CleanRunResult> = {}): CleanRunResult => ({
  summary: "",
  per_target: [],
  outcome: { deleted: 5, freed: 2048, failed: 0 },
  ...over,
});

describe("humanSize", () => {
  it("与后端同口径（1024 进制）", () => {
    expect(humanSize(0)).toBe("0 B");
    expect(humanSize(512)).toBe("512 B");
    expect(humanSize(1024)).toBe("1.0 KB");
    expect(humanSize(1024 * 1024)).toBe("1.0 MB");
    expect(humanSize(1024 ** 4)).toBe("1.0 TB");
  });

  it("大数值不显示多余小数", () => {
    expect(humanSize(500 * 1024 * 1024)).toBe("500 MB");
  });
});

describe("usageColor", () => {
  it("三档映射", () => {
    expect(usageColor("ok")).toBe("green");
    expect(usageColor("warn")).toBe("orange");
    expect(usageColor("err")).toBe("red");
    expect(usageColor("?")).toBe("default");
  });
});

describe("driveLabel", () => {
  it("有卷标时拼上，没有时只有盘符", () => {
    expect(driveLabel(drive())).toBe("C: System");
    expect(driveLabel(drive({ label: "" }))).toBe("C:");
  });
});

describe("needsSpaceWarning", () => {
  it("固定盘 ≥90% 告警", () => {
    expect(needsSpaceWarning(drive({ used_percent: 90 }))).toBe(true);
    expect(needsSpaceWarning(drive({ used_percent: 95 }))).toBe(true);
    expect(needsSpaceWarning(drive({ used_percent: 89.9 }))).toBe(false);
  });

  it("可移动盘不告警（U 盘经常是满的，且不是系统盘）", () => {
    expect(needsSpaceWarning(drive({ used_percent: 99, removable: true }))).toBe(false);
  });
});

describe("diskSummary", () => {
  it("报出最紧张的固定盘", () => {
    const rows = [
      drive({ letter: "C:", used_percent: 60, free_total: 500 }),
      drive({ letter: "D:", used_percent: 95, free_total: 10 }),
      drive({ letter: "E:", used_percent: 99, removable: true, free_total: 0 }),
    ];
    const s = diskSummary(rows);
    expect(s).toContain("2 个本地卷");
    expect(s).toContain("D:");
    expect(s).toContain("95%");
    expect(s).not.toContain("E:", );
  });

  it("没有固定盘时明确说明", () => {
    expect(diskSummary([drive({ removable: true })])).toBe("没有本地固定磁盘");
    expect(diskSummary([])).toBe("没有本地固定磁盘");
  });
});

describe("validateClean", () => {
  it("未勾选必须被拦住", () => {
    const v = validateClean([]);
    expect(v.ok).toBe(false);
    expect(v.reason).toContain("勾选");
  });

  it("勾选了就通过", () => {
    expect(validateClean(["user_temp"]).ok).toBe(true);
  });
});

describe("selectedBytes", () => {
  it("只累加选中的项", () => {
    const rows = [
      target({ key: "a", bytes: 100 }),
      target({ key: "b", bytes: 200 }),
      target({ key: "c", bytes: 400 }),
    ];
    expect(selectedBytes(rows, ["a", "c"])).toBe(500);
    expect(selectedBytes(rows, [])).toBe(0);
    expect(selectedBytes(rows, ["不存在"])).toBe(0);
  });

  it("bytes 缺失时按 0 算（不产生 NaN）", () => {
    const rows = [target({ key: "a", bytes: undefined, human: undefined })];
    expect(selectedBytes(rows, ["a"])).toBe(0);
  });
});

describe("cleanConfirmText", () => {
  it("列出名称、预计释放、不可逆与跳过行为", () => {
    const rows = [target({ key: "a", title: "用户临时文件", bytes: 1024 })];
    const t = cleanConfirmText(rows, ["a"]);
    expect(t).toContain("用户临时文件");
    expect(t).toContain("1.0 KB");
    expect(t).toContain("不可撤销");
    expect(t).toContain("不会删除目录");
    expect(t).toContain("跳过");
  });
});

describe("结果诚实性", () => {
  it("有跳过时 hasSkipped 为真", () => {
    expect(hasSkipped(runResult({ outcome: { deleted: 1, freed: 1, failed: 3 } }))).toBe(true);
    expect(hasSkipped(runResult())).toBe(false);
  });

  it("摘要必须汇报跳过数，并说明这是正常的", () => {
    const s = cleanResultSummary(
      runResult({ outcome: { deleted: 3, freed: 2048, failed: 7 } }),
    );
    expect(s).toContain("3 个");
    expect(s).toContain("2.0 KB");
    expect(s).toContain("7 个");
    expect(s).toContain("跳过");
    expect(s).toMatch(/占用|正常/);
  });

  it("没有跳过时摘要不提跳过（避免噪声）", () => {
    const s = cleanResultSummary(runResult());
    expect(s).not.toContain("跳过");
  });

  it("outcome 缺失时不崩", () => {
    const broken = { summary: "", per_target: [] } as unknown as CleanRunResult;
    expect(cleanResultSummary(broken)).toContain("0 个");
    expect(hasSkipped(broken)).toBe(false);
  });
});

describe("targetResultText", () => {
  it("普通目标报文件数与释放量", () => {
    const t = targetResultText({
      key: "user_temp",
      title: "x",
      ok: true,
      deleted: 12,
      freed: 2048,
      failed: 0,
      message: "",
    });
    expect(t).toContain("12");
    expect(t).toContain("2.0 KB");
  });

  it("有跳过时附上跳过数", () => {
    const t = targetResultText({
      key: "user_temp",
      title: "x",
      ok: true,
      deleted: 1,
      freed: 1024,
      failed: 4,
      message: "",
    });
    expect(t).toContain("跳过 4");
  });

  it("回收站用后端消息（没有可统计的字节数）", () => {
    const t = targetResultText({
      key: "recycle",
      title: "回收站",
      ok: true,
      message: "回收站已清空（无法统计释放大小）",
    });
    expect(t).toContain("回收站已清空");
    expect(t).not.toContain("0 个文件");
  });
});

describe("勾选辅助", () => {
  const rows = [
    target({ key: "a", bytes: 1024 }),
    target({ key: "b", bytes: 0, human: "0 B" }),
    target({ key: "c", bytes: 4096 }),
  ];

  it("allTargetKeys 返回全部 key", () => {
    expect(allTargetKeys(rows)).toEqual(["a", "b", "c"]);
    expect(allTargetKeys([])).toEqual([]);
  });

  it("meaningfulKeys 只留有内容的（清 0 字节目标毫无收益）", () => {
    expect(meaningfulKeys(rows)).toEqual(["a", "c"]);
    // bytes 缺失按 0 算
    expect(meaningfulKeys([target({ key: "x", bytes: undefined })])).toEqual([]);
  });

  it("toggleKey 翻转单行选中", () => {
    expect(toggleKey([], "a")).toEqual(["a"]);
    expect(toggleKey(["a", "b"], "a")).toEqual(["b"]);
    // 不去重输入、不改原数组顺序语义：重复选中时只留一个
    expect(toggleKey(["a"], "a")).toEqual([]);
  });
});
