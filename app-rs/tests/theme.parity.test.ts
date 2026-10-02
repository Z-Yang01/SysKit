/**
 * 跨语言主题奇偶校验：解析 syskit/ui/theme.py 的 _LIGHT/_DARK，
 * 与本侧 TS token 逐值比对——Python 版退役前两边不允许视觉分叉。
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { DARK, LIGHT, TOKEN_NAMES, type Palette } from "../src/theme";

const REPO = resolve(__dirname, "..", "..");

function parse_py_palette(dict_name: string): Palette {
  const src = readFileSync(resolve(REPO, "syskit", "ui", "theme.py"), "utf-8");
  const start = src.indexOf(`${dict_name} = {`);
  if (start < 0) throw new Error(`theme.py 中找不到 ${dict_name}`);
  const end = src.indexOf("}", start);
  const body = src.slice(start, end + 1);
  const out = {} as Palette;
  for (const m of body.matchAll(/"([a-z_]+)":\s*"(#[0-9a-fA-F]{6})"/g)) {
    out[m[1] as keyof Palette] = m[2];
  }
  return out;
}

describe("theme parity with syskit/ui/theme.py", () => {
  it("token 名单一致（32 个，无多无少）", () => {
    const pyLight = parse_py_palette("_LIGHT");
    expect(TOKEN_NAMES).toHaveLength(32);
    expect(Object.keys(pyLight).sort()).toEqual([...TOKEN_NAMES].sort());
  });

  it("_LIGHT 逐值一致", () => {
    const py = parse_py_palette("_LIGHT");
    for (const name of TOKEN_NAMES) {
      expect(py[name], `LIGHT.${name}`).toBe(LIGHT[name]);
    }
  });

  it("_DARK 逐值一致", () => {
    const py = parse_py_palette("_DARK");
    for (const name of TOKEN_NAMES) {
      expect(py[name], `DARK.${name}`).toBe(DARK[name]);
    }
  });
});
