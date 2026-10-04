/**
 * 跨语言设置奇偶校验：解析 `syskit/config.py` 的 `DEFAULTS`，与本侧默认值表逐键比对。
 *
 * 为什么必须有这道锁：两版共用 `%APPDATA%\SysKit\settings.json`，任一版把默认值
 * 或键名改错（少一个键、默认值从 50 变 30、bool 写成 int）都会让另一版行为分叉，
 * 而且是**静默**分叉。与 `theme.parity.test.ts` 同款思路：源码文本即契约。
 *
 * 说明：Python 的 DEFAULTS 里没有 `start_with_windows`（死键已删，注释可证），
 * 因此期望键集 = 解析出的键集，不允许「多也无所谓」。
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const REPO = resolve(__dirname, "..", "..");

/** Rust 侧 `settings.rs::defaults()` 的镜像（键 → 字面量文本 + 类型）。 */
type Kind = "bool" | "int" | "str";
const RUST_DEFAULTS: Record<string, { literal: string; kind: Kind }> = {
  auto_repair_enabled: { literal: "True", kind: "bool" },
  poll_interval_sec: { literal: "30", kind: "int" },
  auto_repair_max_level: { literal: "3", kind: "int" },
  fail_threshold: { literal: "2", kind: "int" },
  cooldown_sec: { literal: "180", kind: "int" },
  level3_cooldown_sec: { literal: "300", kind: "int" },
  minimize_to_tray: { literal: "True", kind: "bool" },
  tray_rate_enabled: { literal: "True", kind: "bool" },
  sound_alert_enabled: { literal: "True", kind: "bool" },
  dark_mode: { literal: "False", kind: "bool" },
  dns_preset: { literal: "阿里", kind: "str" },
  clipboard_max: { literal: "50", kind: "int" },
  clipboard_watch: { literal: "False", kind: "bool" },
  clipboard_retain_days: { literal: "1", kind: "int" },
  clip_process_blacklist: { literal: "keepass,bitwarden,1password,enpass", kind: "str" },
  clip_sensitive_ttl: { literal: "25", kind: "int" },
  clip_capture_images: { literal: "True", kind: "bool" },
  vault_autolock_min: { literal: "30", kind: "int" },
  vault_trust_min: { literal: "30", kind: "int" },
  palette_hotkey: { literal: "Ctrl+Alt+K", kind: "str" },
  clip_paste_hotkey: { literal: "Ctrl+Win+V", kind: "str" },
  clip_quick_hotkey: { literal: "Ctrl+Alt+V", kind: "str" },
  migrated_v2: { literal: "False", kind: "bool" },
  palette_commands: { literal: "", kind: "str" },
};

interface PyEntry {
  raw: string;
  kind: Kind;
}

/** 从 config.py 里抽出 DEFAULTS = {...} 块的每个 `"key": <字面量>,` 条目。 */
function parsePyDefaults(): Map<string, PyEntry> {
  const src = readFileSync(resolve(REPO, "syskit", "config.py"), "utf-8");
  const start = src.indexOf("DEFAULTS = {");
  if (start < 0) throw new Error("config.py 中找不到 DEFAULTS");
  const open = src.indexOf("{", start);
  if (open < 0) throw new Error("DEFAULTS 缺少左花括号");

  // 按花括号配平找到 DEFAULTS 的右花括号（后一个字典 NCSI_STOCK 也以 "}" 收尾，
  // 用 indexOf("\n}") 会提前截断 —— 那正是本测试第一版的真实 bug）
  let depth = 0;
  let close = -1;
  for (let i = open; i < src.length; i++) {
    if (src[i] === "{") depth += 1;
    else if (src[i] === "}") {
      depth -= 1;
      if (depth === 0) {
        close = i;
        break;
      }
    }
  }
  if (close < 0) throw new Error("DEFAULTS 块未正常结束");
  const body = src.slice(open + 1, close);

  const out = new Map<string, PyEntry>();
  // 只认「引号键 + 冒号」这种形态；值里的引号字符串（如 "阿里"）不带冒号
  const re = /"([^"]+)"\s*:\s*("[^"]*"|[A-Za-z]+|\d+)\s*,?/g;
  for (const m of body.matchAll(re)) {
    const [, key, valueText] = m;
    const v = valueText.trim();
    let entry: PyEntry;
    if (v === "True" || v === "False") {
      entry = { raw: v, kind: "bool" };
    } else if (/^-?\d+$/.test(v)) {
      entry = { raw: v, kind: "int" };
    } else if (v.startsWith('"') && v.endsWith('"')) {
      entry = { raw: v.slice(1, -1), kind: "str" };
    } else {
      throw new Error(`DEFAULTS 中 ${key} 的值形态无法识别: ${v}`);
    }
    out.set(key, entry);
  }
  return out;
}

const PY = parsePyDefaults();

describe("settings parity with syskit/config.py", () => {
  it("解析到全部键（解析器本身有效）", () => {
    expect(PY.size).toBe(24);
  });

  it("键集完全一致：无多、无少（含已删死键不得复活）", () => {
    expect([...PY.keys()].sort()).toEqual(Object.keys(RUST_DEFAULTS).sort());
    expect(PY.has("start_with_windows")).toBe(false);
  });

  it("逐键默认值与类型一致", () => {
    for (const [key, rust] of Object.entries(RUST_DEFAULTS)) {
      const py = PY.get(key);
      expect(py, `config.py 缺少 ${key}`).toBeDefined();
      expect(py!.kind, `${key} 类型`).toBe(rust.kind);
      expect(py!.raw, `${key} 默认值`).toBe(rust.literal);
    }
  });

  it("热键默认值与本版一致（M1 硬契约）", () => {
    expect(PY.get("palette_hotkey")?.raw).toBe("Ctrl+Alt+K");
    expect(PY.get("clip_paste_hotkey")?.raw).toBe("Ctrl+Win+V");
    expect(PY.get("clip_quick_hotkey")?.raw).toBe("Ctrl+Alt+V");
  });

  it("剪贴板默认关（opt-in 语义不得被改回默认开）", () => {
    expect(PY.get("clipboard_watch")?.raw).toBe("False");
  });
});
