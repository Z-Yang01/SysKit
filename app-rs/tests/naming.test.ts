/**
 * 前端命名约定守卫（vitest 执行）。
 *
 * ## 为什么需要这个测试
 *
 * Windows 文件系统大小写不敏感，但 TypeScript 的大小写敏感解析会把
 * `SettingsPage.tsx` 与 `settingsPage.ts` 判为**同一个文件的两种拼写**，
 * 报 TS1149/TS1261 而不是"文件不存在"——错误信息很绕，容易浪费一轮。
 *
 * 这个坑在本项目已经踩过三次：
 * - `Hosts.tsx` vs `hosts.ts` → 组件改名 `ToolboxHosts.tsx`
 * - `Dashboard.tsx` vs `dashboard.ts` → 组件改名 `Overview.tsx`
 * - `SettingsPage.tsx` vs `settingsPage.ts` → 组件改名 `SettingsView.tsx`
 *
 * 与其每次踩了再改，不如把规则写成可执行的断言：
 * **页面组件用 `Xxx.tsx`（首字母大写且整体不是逻辑模块名的变体），
 * 纯逻辑模块用 `xxx.ts`（小写开头），二者不得只差大小写。**
 */
import { readdirSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const SRC = join(__dirname, "..", "src");

function files(ext: string): string[] {
  return readdirSync(SRC).filter((f) => f.endsWith(ext));
}

describe("文件名不得只差大小写（Windows 大小写不敏感）", () => {
  it("所有源文件的小写名互不冲突", () => {
    const all = [...files(".ts"), ...files(".tsx")];
    const byLower = new Map<string, string[]>();
    for (const f of all) {
      const k = f.toLowerCase();
      byLower.set(k, [...(byLower.get(k) ?? []), f]);
    }
    const clashes = [...byLower.values()].filter((v) => v.length > 1);
    expect(
      clashes,
      `这些文件名只差大小写，Windows 上会互相覆盖/报 TS1149：${JSON.stringify(clashes)}`,
    ).toEqual([]);
  });

  it("逻辑模块（.ts）与组件（.tsx）不得同名异大小写", () => {
    // 去掉扩展名后比较：foo.ts 与 Foo.tsx 会撞
    const stem = (f: string) => f.replace(/\.tsx?$/, "").toLowerCase();
    const ts = new Set(files(".ts").map(stem));
    const tsx = files(".tsx").map(stem);
    const bad = tsx.filter((s) => ts.has(s));
    expect(bad, `组件与逻辑模块同名（只差大小写）：${JSON.stringify(bad)}`).toEqual([]);
  });

  it("页面组件一律用 .tsx 且以大写字母开头", () => {
    // main.tsx 是 Vite 入口（不是组件），按约定小写；这是唯一豁免
    const ENTRY = new Set(["main.tsx"]);
    for (const f of files(".tsx")) {
      if (ENTRY.has(f)) continue;
      const head = f[0];
      expect(head, `${f} 应以大写字母开头（组件约定）`).toBe(head.toUpperCase());
    }
  });
});
