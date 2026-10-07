/**
 * 前后端契约静态审计（vitest，Node 侧读源码）。
 *
 * 接线守卫（commands.rs::every_tauri_command_is_registered_in_lib）只保证
 * "Rust 声明的命令都注册了"；本文件补上另一半：
 *   1. 前端 invoke 的每个命令名都在注册清单里（抓 invoke 拼写错）；
 *   2. 每个 invoke 点传的参数键都能对上 Rust 形参名（抓参数名漂移——
 *      Tauri 只在运行时按名匹配，键拼错 = 用户点按钮才炸，测试全绿）；
 *   3. Rust 侧非 Option 必填形参在每个 invoke 点都有传（抓漏传）。
 *   4. tools/*.bat 脚本卫生：恒 ASCII + 恒 CRLF（cmd 以 OEM 代码页
 *      (GBK) 解析批处理，UTF-8 中文注释炸解析——89a0dc1 实锤后立规，
 *      同日 9464def 又引入中文注释，轮次110 回归锁固化）。
 *
 * Tauri v2 默认把 JS 侧 camelCase 键映射到 Rust snake_case 形参；
 * 本仓前端混用两种风格，比较前统一归一化到 snake_case。
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";

const REPO = resolve(__dirname, "..");
const SRC = join(REPO, "src");
const TAURI = join(REPO, "src-tauri", "src");

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) walk(p, out);
    else if (/\.(tsx?|rs)$/.test(name)) out.push(p);
  }
  return out;
}

function read(p: string): string {
  return readFileSync(p, "utf-8");
}

/** camelCase / 原样 → snake_case（连续大写与已有下划线均保持稳定）。 */
function toSnake(s: string): string {
  return s
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1_$2")
    .toLowerCase();
}

interface InvokeSite {
  cmd: string;
  args: Set<string>; // 已归一化为 snake_case
  file: string;
}

/** 从前端源码提取全部 invoke 点（命令名 + 实参键，含跨行对象字面量）。 */
function collectInvokeSites(): InvokeSite[] {
  const sites: InvokeSite[] = [];
  for (const p of walk(SRC)) {
    if (!/\.(tsx?|)$/.test(p)) continue;
    const src = read(p);
    const re = /invoke(?:<[^>]*>)?\(\s*"([a-z_]+)"\s*,\s*\{/g;
    let m: RegExpExecArray | null;
    while ((m = re.exec(src))) {
      const cmd = m[1];
      // 平衡括号找对象字面量结束
      let depth = 1;
      let i = m.index + m[0].length;
      while (i < src.length && depth > 0) {
        const ch = src[i];
        if (ch === "{") depth++;
        else if (ch === "}") depth--;
        i++;
      }
      const body = "{" + src.slice(m.index + m[0].length, i - 1); // 补回被 m[0] 吃掉的起始 {
      // 去掉字符串字面量与注释，避免键名误报
      const clean = body
        .replace(/`[^`]*`/g, "")
        .replace(/"[^"\n]*"/g, "")
        .replace(/'[^'\n]*'/g, "")
        .replace(/\/\/[^\n]*/g, "");
      const args = new Set<string>();
      // key: value 形式
      for (const k of clean.matchAll(/([{,]\s*)([A-Za-z_][A-Za-z0-9_]*)\s*:/g)) {
        args.add(toSnake(k[2]));
      }
      // 简写 { keyword } / 尾随 { keyword }
      for (const k of clean.matchAll(/[{,]\s*([A-Za-z_][A-Za-z0-9_]*)\s*[,}]/g)) {
        args.add(toSnake(k[1]));
      }
      sites.push({ cmd, args, file: p.replace(REPO + "\\", "").replace(REPO + "/", "") });
    }
  }
  return sites;
}

/** Rust 侧：已注册命令名 + 每个命令的形参（归一化 snake_case，标注是否 Option）。 */
interface RustCmd {
  params: Map<string, boolean>; // snake_name -> optional(Option<>/缺省)
  structShaped: boolean; // 单 CamelCase 整包参数（对象键映射结构体字段，不做键名比对）
}

function collectRustCommands(): { registered: Set<string>; cmds: Map<string, RustCmd> } {
  // 注册清单（generate_handler![...]）
  const lib = read(join(TAURI, "lib.rs"));
  const zone = lib.split("generate_handler!")[1] ?? "";
  const zoneEnd = zone.indexOf("]");
  const registered = new Set<string>();
  for (const seg of zone.slice(0, zoneEnd).split("commands::").slice(1)) {
    const name = seg.match(/[a-zA-Z0-9_]+/)?.[0];
    if (name) registered.add(name);
  }

  const cmds = new Map<string, RustCmd>();
  const src = read(join(TAURI, "commands.rs"));
  const chunks = src.split("#[tauri::command]");
  for (const chunk of chunks.slice(1)) {
    const sig = chunk.match(
      /pub\s+(?:async\s+)?fn\s+([a-z_][a-z0-9_]*)(?:<[^>(]*>)?\s*\(([^)]*)\)/,
    );
    if (!sig) continue;
    const name = sig[1];
    const params = new Map<string, boolean>();
    let structShaped = false; // 单个 CamelCase 整包参数（如 req: PasteRequest）：对象键映射结构体字段
    for (const raw of sig[2].split(",")) {
      const part = raw.trim();
      if (!part) continue;
      const pm = part.match(/([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.+?)$/);
      if (!pm) continue; // 如 self / 模式参数
      const [, pname, ptype] = pm;
      if (/tauri::State|State<'_|AppHandle|Window|WebviewWindow/.test(ptype)) continue;
      if (/^[A-Z]/.test(ptype.trim()) && !ptype.includes("::")) {
        structShaped = true;
        continue;
      }
      const optional = /Option</.test(ptype);
      params.set(toSnake(pname), optional);
    }
    cmds.set(name, { params, structShaped });
  }
  return { registered, cmds };
}

let sites: InvokeSite[] = [];
let { registered, cmds } = {} as ReturnType<typeof collectRustCommands>;

beforeAll(() => {
  sites = collectInvokeSites();
  ({ registered, cmds } = collectRustCommands());
});

describe("前后端 invoke 契约", () => {
  it("样本量合理（解析器没有失效成空转）", () => {
    expect(sites.length).toBeGreaterThanOrEqual(60);
    expect(registered.size).toBeGreaterThanOrEqual(100);
  });

  it("前端 invoke 的每个命令名都已注册", () => {
    const bad = sites.filter((s) => !registered.has(s.cmd));
    expect(
      bad.map((s) => `${s.cmd} @ ${s.file}`),
    ).toEqual([]);
  });

  it("每个 invoke 点的参数键都能对上 Rust 形参（抓键名漂移/拼写错）", () => {
    const problems: string[] = [];
    for (const s of sites) {
      const def = cmds.get(s.cmd);
      if (!def) {
        problems.push(`${s.cmd} @ ${s.file}: Rust 侧未找到该命令签名`);
        continue;
      }
      if (def.structShaped) continue;
      for (const a of s.args) {
        if (!def.params.has(a)) {
          problems.push(
            `${s.cmd} @ ${s.file}: 实参 \`${a}\` 不在形参清单 (${[...def.params.keys()].join(", ") || "无"})`,
          );
        }
      }
    }
    expect(problems).toEqual([]);
  });

  it("Rust 必填形参（非 Option）在每个 invoke 点都有传", () => {
    const problems: string[] = [];
    for (const s of sites) {
      const def = cmds.get(s.cmd);
      if (!def || def.structShaped) continue;
      for (const [p, optional] of def.params) {
        if (!optional && !s.args.has(p)) {
          problems.push(`${s.cmd} @ ${s.file}: 必填形参 \`${p}\` 未传`);
        }
      }
    }
    expect(problems).toEqual([]);
  });
});

describe("tools/*.bat 脚本卫生（cmd 按 OEM 代码页解析批处理）", () => {
  // cmd 逐行按 OEM 代码页（本机 GBK）读 .bat：UTF-8 中文注释曾炸解析
  // （89a0dc1：'oy'/'?rem' 不是内部或外部命令），行尾多字节序列还会在
  // GBK 视角留下孤儿前导字节、令整文件不可 GBK 解码。同日 9464def 又
  // 把中文注释写了回来——这里把「恒 ASCII + 恒 CRLF」锁成断言，两条
  // 各抓一类退化（任何一条红都说明有人往 bat 里写了非 ASCII / 转了 LF）。
  const TOOLS = join(REPO, "..", "tools");
  const bats = readdirSync(TOOLS).filter((f) => f.endsWith(".bat"));

  it("tools/*.bat 全部纯 ASCII（禁 UTF-8 中文注释）", () => {
    expect(bats.length).toBeGreaterThan(0); // 守卫不能因文件改名而静默空转
    const bad: string[] = [];
    for (const f of bats) {
      const bytes = readFileSync(join(TOOLS, f));
      if (bytes.some((b) => b > 0x7f)) bad.push(f);
    }
    expect(bad).toEqual([]);
  });

  it("tools/*.bat 全部 CRLF 行尾（每个 LF 前必须有 CR）", () => {
    const bad: string[] = [];
    for (const f of bats) {
      const bytes = readFileSync(join(TOOLS, f));
      for (let i = 0; i < bytes.length; i++) {
        if (bytes[i] === 0x0a && (i === 0 || bytes[i - 1] !== 0x0d)) {
          bad.push(f);
          break;
        }
      }
    }
    expect(bad).toEqual([]);
  });
});
