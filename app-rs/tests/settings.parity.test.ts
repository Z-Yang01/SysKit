/**
 * 设置默认值冻结契约：解析 `src-tauri/src/settings.rs::defaults()` 源码，
 * 与本文件的 FROZEN_DEFAULTS 快照逐键比对。
 *
 * 历史：Python 版退役前，本测试解析 `syskit/config.py` 的 DEFAULTS 做跨语言
 * 奇偶校验（两版共用一份 settings.json，静默分叉是真实事故源）。Python 版
 * 归档后契约本体固化为下方快照表——值 = python-final 归档时的对齐终态。
 * 任何对 defaults() 的改动（加键/删键/改默认值/改类型）都必须有意识地
 * 更新快照并在提交说明里写明理由，否则此测试红。
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

/** python-final 对齐终态 + v1.3 新增 clip_sensitive_detect（25 键）。键序无关。 */
type Kind = "bool" | "int" | "str";
const FROZEN_DEFAULTS: Record<string, { literal: string; kind: Kind }> = {
  auto_repair_enabled: { literal: "true", kind: "bool" },
  poll_interval_sec: { literal: "30", kind: "int" },
  auto_repair_max_level: { literal: "3", kind: "int" },
  fail_threshold: { literal: "2", kind: "int" },
  cooldown_sec: { literal: "180", kind: "int" },
  level3_cooldown_sec: { literal: "300", kind: "int" },
  minimize_to_tray: { literal: "true", kind: "bool" },
  tray_rate_enabled: { literal: "true", kind: "bool" },
  sound_alert_enabled: { literal: "true", kind: "bool" },
  dark_mode: { literal: "false", kind: "bool" },
  dns_preset: { literal: "阿里", kind: "str" },
  clipboard_max: { literal: "1000", kind: "int" },
  clipboard_watch: { literal: "false", kind: "bool" },
  clipboard_retain_days: { literal: "30", kind: "int" },
  clip_process_blacklist: { literal: "keepass,bitwarden,1password,enpass", kind: "str" },
  clip_sensitive_ttl: { literal: "25", kind: "int" },
  clip_sensitive_detect: { literal: "true", kind: "bool" },
  clip_capture_images: { literal: "true", kind: "bool" },
  vault_autolock_min: { literal: "30", kind: "int" },
  vault_trust_min: { literal: "30", kind: "int" },
  palette_hotkey: { literal: "Ctrl+Alt+K", kind: "str" },
  clip_paste_hotkey: { literal: "Ctrl+Win+V", kind: "str" },
  clip_quick_hotkey: { literal: "Ctrl+Alt+V", kind: "str" },
  migrated_v2: { literal: "false", kind: "bool" },
  palette_commands: { literal: "", kind: "str" },
};

interface RsEntry {
  literal: string;
  kind: Kind;
}

/** 从 settings.rs 抽出 defaults() 的 vec![...] 块，解析每个 ("key", Value::…, Ty) 条目。 */
function parseRsDefaults(): Map<string, RsEntry> {
  const src = readFileSync(
    resolve(__dirname, "..", "src-tauri", "src", "settings.rs"),
    "utf-8",
  );
  const fnAt = src.indexOf("pub fn defaults()");
  if (fnAt < 0) throw new Error("settings.rs 中找不到 pub fn defaults()");
  const open = src.indexOf("vec![", fnAt);
  if (open < 0) throw new Error("defaults() 缺少 vec![");

  // 按方括号配平找到 vec! 的收尾（条目里有元组括号，只数方括号即可）
  let depth = 0;
  let close = -1;
  for (let i = open + 4; i < src.length; i++) {
    if (src[i] === "[") depth += 1;
    else if (src[i] === "]") {
      depth -= 1;
      if (depth === 0) {
        close = i;
        break;
      }
    }
  }
  if (close < 0) throw new Error("defaults() 的 vec![ 未正常结束");
  const body = src.slice(open + 5, close);

  const out = new Map<string, RsEntry>();
  // 三种值形态：Value::Bool(true/false) / Value::from(整数) / Value::from("字符串")
  // 尾随逗号可选：单行条目 rustfmt 为 `Bool),`，多行条目为 `Bool,\n        )`
  const re =
    /\(\s*"([^"]+)"\s*,\s*Value::(?:Bool\((true|false)\)|from\((-?\d+)\)|from\("([^"]*)"\))\s*,\s*(Bool|Int|Str)\s*,?\s*\)/gs;
  for (const m of body.matchAll(re)) {
    const [, key, b, n, s, ty] = m;
    const entry: RsEntry =
      b !== undefined
        ? { literal: b, kind: "bool" }
        : n !== undefined
          ? { literal: n, kind: "int" }
          : { literal: s!, kind: "str" };
    const kind: Kind = ty === "Bool" ? "bool" : ty === "Int" ? "int" : "str";
    expect(entry.kind, `${key} 值形态与类型标注不一致`).toBe(kind);
    out.set(key, entry);
  }
  return out;
}

const RS = parseRsDefaults();

describe("settings defaults 冻结契约（settings.rs vs 快照）", () => {
  it("解析到全部键（解析器本身有效）", () => {
    expect(RS.size).toBe(25);
  });

  it("键集完全一致：无多、无少（死键不得复活）", () => {
    expect([...RS.keys()].sort()).toEqual(Object.keys(FROZEN_DEFAULTS).sort());
    expect(RS.has("start_with_windows")).toBe(false);
  });

  it("逐键默认值与类型一致", () => {
    for (const [key, frozen] of Object.entries(FROZEN_DEFAULTS)) {
      const rs = RS.get(key);
      expect(rs, `settings.rs 缺少 ${key}`).toBeDefined();
      expect(rs!.kind, `${key} 类型`).toBe(frozen.kind);
      expect(rs!.literal, `${key} 默认值`).toBe(frozen.literal);
    }
  });

  it("热键默认值（M1 硬契约）", () => {
    expect(RS.get("palette_hotkey")?.literal).toBe("Ctrl+Alt+K");
    expect(RS.get("clip_paste_hotkey")?.literal).toBe("Ctrl+Win+V");
    expect(RS.get("clip_quick_hotkey")?.literal).toBe("Ctrl+Alt+V");
  });

  it("剪贴板默认关（opt-in 语义不得被改回默认开）", () => {
    expect(RS.get("clipboard_watch")?.literal).toBe("false");
  });
});
