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
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
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
  /** 实参对象用了 ...展开（内容文本上不可验证）——必填形参检查对它豁免 */
  spread: boolean;
}

/** 跳过从 open 开始的字符串字面量，返回收尾引号之后的位置（未收尾到文件尾）。 */
function skipString(src: string, open: number): number {
  const q = src[open];
  let i = open + 1;
  while (i < src.length) {
    if (src[i] === q) {
      // 前面连续偶数个反斜杠才是真收尾（"a\"b" 的引号是转义，不是收尾）
      let bs = 0;
      for (let j = i - 1; j >= 0 && src[j] === "\\"; j--) bs++;
      if (bs % 2 === 0) return i + 1;
    }
    i++;
  }
  return src.length;
}

/**
 * 从 from（对象起始 { 之后）扫描平衡大括号，返回收尾 } 之后的位置。
 * 字符串字面量（` " '）与注释（// 、块注释）里的大括号**不参与配平**——
 * 裸扫描曾把 `label: "}"` 的字符串收尾 } 当成对象收尾，实参被截断，
 * 必填形参误报「未传」；注释里的 } 更会让实参提取成空（轮次112 实锤）。
 */
function objectEnd(src: string, from: number): number {
  let depth = 1;
  let i = from;
  while (i < src.length) {
    const ch = src[i];
    if (ch === '"' || ch === "'" || ch === "`") {
      i = skipString(src, i);
      continue;
    }
    if (ch === "/" && src[i + 1] === "/") {
      const nl = src.indexOf("\n", i + 2);
      if (nl < 0) return src.length;
      i = nl;
      i++;
      continue;
    }
    if (ch === "/" && src[i + 1] === "*") {
      const end = src.indexOf("*/", i + 2);
      i = end < 0 ? src.length : end + 2;
      continue;
    }
    if (ch === "{") depth++;
    else if (ch === "}") {
      depth--;
      if (depth === 0) return i + 1;
    }
    i++;
  }
  return src.length; // 未配平（残缺源码）：兜底拿到文件尾
}

/** 从前端源码提取全部 invoke 点（命令名 + 实参键，含跨行对象字面量）。 */
function collectInvokeSites(dir: string = SRC): InvokeSite[] {
  const sites: InvokeSite[] = [];
  for (const p of walk(dir)) {
    if (!/\.(tsx?|)$/.test(p)) continue;
    const src = read(p);
    const re = /invoke(?:<[^>]*>)?\(\s*"([a-z_]+)"\s*,\s*\{/g;
    let m: RegExpExecArray | null;
    while ((m = re.exec(src))) {
      const cmd = m[1];
      const end = objectEnd(src, m.index + m[0].length);
      // 补回被 m[0] 吃掉的起始 {，并保留收尾 }：收尾括号被截掉时，
      // 简写 { dir } 的最后一个键没有尾随 [,}] 可匹配，整个实参对象会被
      // 提取成空——「必填形参未传」永远查不出来（轮次111 变异验证实锤）
      const body = "{" + src.slice(m.index + m[0].length, end);
      // 去掉字符串字面量与注释，避免键名误报（与 objectEnd 同一套识别规则）
      const clean = body
        .replace(/`[^`]*`/g, "")
        .replace(/"[^"\n]*"/g, "")
        .replace(/'[^'\n]*'/g, "")
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .replace(/\/\/[^\n]*/g, "");
      const args = new Set<string>();
      // key: value 形式
      for (const k of clean.matchAll(/([{,]\s*)([A-Za-z_][A-Za-z0-9_]*)\s*:/g)) {
        args.add(toSnake(k[2]));
      }
      // 简写 { keyword } / 尾随 { keyword }。收尾必须用前瞻 (?=[,}])：
      // 消费型 [,}] 会把属性间的逗号吃掉，多属性简写永远只抓到前 N-1 个
      // （{ key, value } 提不出 value——轮次111 修复）
      for (const k of clean.matchAll(/[{,]\s*([A-Za-z_][A-Za-z0-9_]*)\s*(?=[,}])/g)) {
        args.add(toSnake(k[1]));
      }
      sites.push({
        cmd,
        args,
        file: p.replace(REPO + "\\", "").replace(REPO + "/", ""),
        spread: /\.\.\./.test(clean),
      });
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
      const t = ptype.trim();
      // 「单个裸 CamelCase 标识符」才可能是整包结构体参数（如 req: PasteRequest）。
      // 泛型容器（Option<bool>/Option<u32>/Vec<i64>/HashMap<K,V>…）首字母同样
      // 大写，但在 invoke 里仍是**具名**形参——轮次111 只排除了 String 族，
      // 仍有 16 条命令（sk_ping/sk_modes_run/sk_log_tail/sk_hosts_write…）被
      // 误判整包、键名+必填审计整条跳过（轮次112 实锤：sk_ping 必填 host 改错
      // 名契约测试照样绿）。泛型必含 `<`，裸标识符判定天然排除；String 是唯一
      // 裸写的大写标量，显式排除。
      if (/^[A-Z][A-Za-z0-9_]*$/.test(t) && t !== "String") {
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

  it("整包结构体豁免只覆盖裸 CamelCase 形参（泛型容器仍逐键审计）", () => {
    // 轮次112 实锤锁：曾把 Option<bool>/Option<u32>/Vec<i64> 误判成整包，
    // sk_ping 等命令的必填形参改错名测试照样绿。这些命令此刻必须处于
    // 「已审计」状态——若此处红了，说明有人把豁免判定放宽了。
    for (const cmd of [
      "sk_ping", // count: Option<u32> + host: String
      "sk_modes_run", // mode: String + open_apps/clear_bg: Option<bool>
      "sk_log_tail", // lines: Option<usize>
      "sk_hosts_write", // backup: Option<bool> + entries: Vec<…>
      "sk_vault_secrets_bulk", // account_ids: Vec<i64>
    ]) {
      const def = cmds.get(cmd);
      expect(def, `${cmd} 签名必须解析得到`).toBeTruthy();
      expect(def!.structShaped, `${cmd} 不得被豁免审计`).toBe(false);
      expect(def!.params.size, `${cmd} 形参清单不得为空`).toBeGreaterThan(0);
    }
    // 真整包结构体（req: PasteRequest，裸 CamelCase）仍豁免
    expect(cmds.get("sk_paste")?.structShaped).toBe(true);
  });

  it("解析器形态回归：多行/简写/展开/字符串与注释内大括号（夹具目录）", () => {
    const dir = mkdtempSync(join(tmpdir(), "contract_probe_"));
    try {
      writeFileSync(
        join(dir, "probe.tsx"),
        [
          `const a = invoke("cmd_multi", {`,
          `  rootPath: p,`,
          `  includeHidden: true,`,
          `});`,
          `const b = invoke("cmd_short", { dir });`,
          `const c = invoke("cmd_spread", { ...base, extra: 1 });`,
          `const d = invoke("cmd_str_close", { label: "a (b) } c", n: 1 });`,
          `const e = invoke("cmd_comment", { // 注释 } 不配平`,
          `  a: 1, b: 2,`,
          `});`,
          `const f = invoke("cmd_str_open", { tmpl: "{x}", k: 2 });`,
          `const g = invoke("cmd_block", { /* 块注释 } */ m: 3 });`,
        ].join("\n"),
      );
      const sites = collectInvokeSites(dir);
      const by = (cmd: string) => sites.find((s) => s.cmd === cmd);
      expect(by("cmd_multi")?.args).toEqual(new Set(["root_path", "include_hidden"]));
      expect(by("cmd_short")?.args).toEqual(new Set(["dir"]));
      expect(by("cmd_spread")?.spread).toBe(true);
      expect(by("cmd_spread")?.args.has("extra")).toBe(true);
      // 字符串/注释里的大括号不参与配平：实参一个不少（轮次112 实锤锁）
      expect(by("cmd_str_close")?.args).toEqual(new Set(["label", "n"]));
      expect(by("cmd_comment")?.args).toEqual(new Set(["a", "b"]));
      expect(by("cmd_str_open")?.args).toEqual(new Set(["tmpl", "k"]));
      expect(by("cmd_block")?.args).toEqual(new Set(["m"]));
      for (const cmd of ["cmd_multi", "cmd_short", "cmd_str_close", "cmd_comment"]) {
        expect(by(cmd)?.spread, cmd).toBe(false);
      }
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
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
      if (s.spread) continue; // ...展开站点的实参清单文本上不可见，无法核对
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
