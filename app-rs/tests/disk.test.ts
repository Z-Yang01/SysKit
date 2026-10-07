/**
 * 磁盘空间与垃圾清理纯逻辑测试。
 *
 * 重点是清理的**安全与诚实**：不可逆操作要被校验拦住，结果必须如实汇报跳过项。
 */
import { describe, expect, it } from "vitest";

import {
  allTargetKeys,
  childIcon,
  cleanConfirmText,
  cleanResultSummary,
  collapsePath,
  diskSummary,
  drillErrorText,
  driveLabel,
  flattenChildRows,
  hasSkipped,
  humanSize,
  meaningfulKeys,
  needsSpaceWarning,
  rootEmptyText,
  selectedBytes,
  sortChildRows,
  targetResultText,
  toggleExpanded,
  toggleKey,
  truncatedNote,
  usageColor,
  validateClean,
  type ChildrenScan,
  type ChildEntry,
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

// ---------------- 目录逐级展开（下钻分析） ----------------

const child = (over: Partial<ChildEntry> = {}): ChildEntry => ({
  name: "sub",
  path: "C:\data\sub",
  size: 0,
  total: 100,
  file_count: 1,
  error_count: 0,
  is_dir: true,
  ...over,
});

const scanOf = (over: Partial<ChildrenScan> = {}): ChildrenScan => ({
  path: "C:\data",
  name: "data",
  size: 3,
  total: 103,
  file_count: 2,
  error_count: 0,
  children: [],
  truncated: false,
  ext_stats: [],
  top_files: [],
  duration_ms: 5,
  cancelled: false,
  ...over,
});

describe("下钻：子项行排序", () => {
  it("按 total 降序，目录与文件混排同列；并列按名称稳定", () => {
    const rows = [
      child({ name: "b", total: 100 }),
      child({ name: "f.txt", total: 500, is_dir: false }),
      child({ name: "z", total: 100 }),
      child({ name: "a", total: 900 }),
    ];
    expect(sortChildRows(rows).map((r) => r.name)).toEqual(["a", "f.txt", "b", "z"]);
  });

  it("不改动入参数组（纯函数）", () => {
    const rows = [child({ name: "b", total: 1 }), child({ name: "a", total: 2 })];
    sortChildRows(rows);
    expect(rows.map((r) => r.name)).toEqual(["b", "a"]);
  });
});

describe("下钻：展开收起状态机（仅内存态）", () => {
  it("toggleExpanded 翻转；collapsePath 收回起态", () => {
    let expanded = toggleExpanded({}, "C:\a");
    expect(expanded).toEqual({ "C:\a": true });
    expanded = toggleExpanded(expanded, "C:\a");
    expect(expanded).toEqual({});
    // 失败回退：收起指定行且不影响其他已展开行
    const two = toggleExpanded(toggleExpanded({}, "C:\a"), "C:\b");
    expect(collapsePath(two, "C:\a")).toEqual({ "C:\b": true });
  });

  it("未展开/收起时不渲染子项；展开且缓存命中才插入缩进子行", () => {
    const root = scanOf({
      children: [child({ name: "sub", path: "C:\data\sub" })],
    });
    const subScan = scanOf({
      path: "C:\data\sub",
      name: "sub",
      children: [child({ name: "deep", path: "C:\data\sub\deep", total: 30 })],
    });
    // 未展开：只有一行
    expect(flattenChildRows(root, {}, {})).toHaveLength(1);
    // 已展开但结果未到：子行位置是 loading 占位
    const expanded = toggleExpanded({}, "C:\data\sub");
    const loading = flattenChildRows(root, expanded, {});
    expect(loading).toHaveLength(2);
    expect(loading[1]).toMatchObject({ kind: "loading", depth: 1, path: "C:\data\sub" });
    // 缓存命中：插入 depth+1 的子行
    const withSub = flattenChildRows(root, expanded, { "C:\data\sub": subScan });
    expect(withSub).toHaveLength(2);
    expect(withSub[1]).toMatchObject({ kind: "row", depth: 1 });
    expect(withSub[1].row?.name).toBe("deep");
    // 收起：子行整体消失
    expect(flattenChildRows(root, collapsePath(expanded, "C:\data\sub"), { "C:\data\sub": subScan })).toHaveLength(1);
    // 嵌套展开：孙辈 depth=2（deep 的扫描结果自带子行 leaf）
    const two = toggleExpanded(expanded, "C:\data\sub\deep");
    const nested = flattenChildRows(root, two, {
      "C:\data\sub": subScan,
      "C:\data\sub\deep": scanOf({
        path: "C:\data\sub\deep",
        children: [child({ name: "leaf", path: "C:\data\sub\deep\leaf" })],
      }),
    });
    expect(nested.map((r) => r.depth)).toEqual([0, 1, 2]);
    expect(nested[2].row?.name).toBe("leaf");
  });

  it("文件行不可展开（flatten 不为其插 loading/子行）", () => {
    const root = scanOf({
      children: [child({ name: "f.txt", path: "C:\data\f.txt", is_dir: false, total: 5 })],
    });
    // 即便状态异常地把文件路径标成展开，也不产生子行
    const weird = { "C:\data\f.txt": true };
    expect(flattenChildRows(root, weird, {})).toHaveLength(1);
  });

  it("root 未扫描时拍平结果恒为空（不渲染幽灵行）", () => {
    expect(flattenChildRows(null, { "C:\a": true }, {})).toEqual([]);
  });

  it("图标区分目录/文件；失败文案带路径", () => {
    expect(childIcon({ is_dir: true })).toBe("📁");
    expect(childIcon({ is_dir: false })).toBe("📄");
    expect(drillErrorText("C:\a", "拒绝访问")).toBe("展开 C:\a 失败：拒绝访问");
  });
});

describe("下钻：不完整结果的如实呈现（轮次112 审计）", () => {
  it("根扫描取消残留不得冒充「空目录」", () => {
    expect(rootEmptyText(null)).toBe("输入路径后点「开始扫描」");
    expect(rootEmptyText(scanOf({ cancelled: true, children: [] }))).toBe(
      "扫描已取消（结果不完整）",
    );
    expect(rootEmptyText(scanOf({}))).toBe("（空目录）");
    expect(rootEmptyText(scanOf({ children: [child()] }))).toBe("（空目录）");
  });

  it("截断提示：根概要行与深层展开行共用同一文案", () => {
    expect(truncatedNote({ truncated: false, children: [] })).toBeNull();
    expect(truncatedNote({ truncated: true, children: [child(), child()] })).toBe(
      "仅显示最大的前 2 项",
    );
  });
});
