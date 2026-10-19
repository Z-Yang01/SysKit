/**
 * 密码库页（M6：数据层 + 加密层 + 条目管理）。
 *
 * 数据层已实现（rusqlite，schema/格式与 Python 版逐字对齐、跨语言双向互开）：
 * 渲染真实条目（站点/账号两级），复制密码/TOTP 走 `sk_vault_copy_secret`——
 * 解密与剪贴板写入都在后端完成（明文不过 JS），并带三重敏感防护。
 * `vault.key` 的创建/解锁/锁定与主密码管理为加密层能力，同样双向验证。
 *
 * 布局对齐 Python 版截图：顶部一排式工具条（左侧状态徽章内联自动锁定倒计时，
 * 右侧解锁/锁定/主密码/+ 网站全部收进同一排，「移除主密码」收纳进 ··· 菜单）；
 * 解锁后主体左右两栏——左栏固定宽网站列表（搜索嵌在卡标题），右栏该站账号表
 * （卡标题为「站点 › 名称（共 N 个账号）[编辑站点]」面包屑形态；密码列恒定
 * 纯掩码（双击复制），显示/复制等行操作全部收进 ··· 菜单——行内不放图标）。
 *
 * v1.4 UI 迁移：脱离 AntD（antd 引用清零；TOTP 动态码由 message.open 气泡
 * 改为页内常驻卡——30s 滚动 + 倒计时 + 复制/关闭，不再会过期消失）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { useFillRowHeight } from "./useFillRow";
import { nonDefaultParams, parseOtpauth } from "./otpauth";
import { invoke } from "@tauri-apps/api/core";
import { useToast } from "./ui/toast";
import { confirm } from "./ui/confirm";
import { Modal as UiModal } from "./ui/modal";
import {
  Badge,
  Button as UiButton,
  Card,
  CardBody,
  CardHeader,
  CardTitle,
  Empty,
  Input as UiInput,
  PasswordInput,
  Table,
  type Column,
} from "./ui";
import { Textarea as UiTextarea } from "./ui";
import { Alert } from "./ui/alert";
import { Dropdown, type MenuItem } from "./ui/dropdown";
import { Radio } from "./ui/checkbox";
import { Select } from "./ui/select";
import { Slider as UiSlider } from "./ui/slider";
import { Checkbox as UiCheckbox } from "./ui/checkbox";
import { Progress as UiProgress } from "./ui/progress";
import { Tooltip as UiTooltip } from "./ui/tooltip";

import {
  PWGEN_DEFAULT_LENGTH,
  PWGEN_MAX_LENGTH,
  PWGEN_MIN_LENGTH,
  accountTitle,
  canSetMasterPassword,
  contactKindLabel,
  contactSummary,
  maskedSecret,
  modeLabel,
  passwordStrength,
  pwgenEntropyBits,
  pwgenMeterPercent,
  pwgenTier,
  pwgenUsable,
  setupConfirmText,
  storageAvailable,
  validateContact,
  vaultPhase,
  CONTACT_KINDS,
  type AccountRow,
  type ContactRow,
  type PwgenOptions,
  type SiteRow,
  type VaultStatus,
} from "./vault";

/**
 * 账号表单内嵌的密码生成面板。
 *
 * 为什么是自持 state 的独立组件：确认弹窗的内容是打开瞬间的静态快照，
 * 父组件不重渲染（openUnlock 注释里的同根源问题）——选项滑杆/勾选要
 * 在弹窗内联动（强度条实时重算），必须让组件自己持有 state 自己重渲染。
 *
 * 纪律：生成的密码在点「保存」前只活在表单输入框里，但已经是凭据材料——
 * 「复制」必须走 `sk_clip_copy_sensitive` 敏感通道（不进本应用历史 / 不进
 * Win+V 与云剪贴板 / 到期清空）。未保存的密码与已保存密码的暴露窗口一致，
 * 没有理由降级到明文通道（TOTP 复制走敏感通道是同一教训，源码守卫锁死）。
 * 生成只填充输入框，不自动保存——保存与否始终由用户点「保存」决定。
 */
function PasswordGenPanel({ onGenerated }: { onGenerated: (pw: string) => void }) {
  const toast = useToast();
  const [length, setLength] = useState(PWGEN_DEFAULT_LENGTH);
  const [upper, setUpper] = useState(true);
  const [lower, setLower] = useState(true);
  const [digits, setDigits] = useState(true);
  const [symbols, setSymbols] = useState(true);
  const [excludeAmbiguous, setExcludeAmbiguous] = useState(false);
  const [busy, setBusy] = useState(false);
  // 最近一次生成结果：仅存活于本弹窗（关闭即丢），复制按钮据此启用
  const [generated, setGenerated] = useState("");

  const opts: PwgenOptions = {
    length,
    upper,
    lower,
    digits,
    symbols,
    exclude_ambiguous: excludeAmbiguous,
  };
  const usable = pwgenUsable(opts);
  const bits = pwgenEntropyBits(opts);
  const tier = pwgenTier(bits);

  const gen = () => {
    setBusy(true);
    // 六个实参全部写 key: value 显式形态——契约审计（tests/contract.test.ts）
    // 的纯简写逐行列表会被它的 shorthand 正则漏采隔行键，显式键名让它稳定可解析
    void invoke<string>("sk_vault_generate_password", {
      length: length,
      upper: upper,
      lower: lower,
      digits: digits,
      symbols: symbols,
      excludeAmbiguous: excludeAmbiguous,
    })
      .then((pw) => {
        setGenerated(pw);
        onGenerated(pw);
      })
      .catch((e) => toast.error(String(e)))
      .finally(() => setBusy(false));
  };

  const copyGenerated = () => {
    // 未保存的生成密码同为凭据材料：必须走敏感通道（见组件头注释）
    void invoke<{ note: string; ttl: number }>("sk_clip_copy_sensitive", { text: generated })
      .then((r) =>
        toast.success(r.ttl > 0 ? `已复制；${r.ttl} 秒后自动清空剪贴板` : "已复制（不会进入历史）"),
      )
      .catch((e) => toast.error(String(e)));
  };

  return (
    <div className="rounded-md border border-sk-border/70 px-2.5 py-2">
      <div className="flex w-full flex-col gap-1.5">
        <div className="flex flex-wrap items-center gap-3">
          <span className="sk-hint">长度 {length}</span>
          <UiSlider
            min={PWGEN_MIN_LENGTH}
            max={PWGEN_MAX_LENGTH}
            value={length}
            onChange={setLength}
            className="w-[180px]"
          />
          <UiCheckbox checked={upper} onChange={(v) => setUpper(v)}>
            大写
          </UiCheckbox>
          <UiCheckbox checked={lower} onChange={(v) => setLower(v)}>
            小写
          </UiCheckbox>
          <UiCheckbox checked={digits} onChange={(v) => setDigits(v)}>
            数字
          </UiCheckbox>
          <UiCheckbox checked={symbols} onChange={(v) => setSymbols(v)}>
            符号
          </UiCheckbox>
          <UiTooltip title="剔除 0/O、1/l/I、竖线、引号等手抄易看错的字符">
            <UiCheckbox
              checked={excludeAmbiguous}
              onChange={(v) => setExcludeAmbiguous(v)}
            >
              排除易混淆字符
            </UiCheckbox>
          </UiTooltip>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <UiButton size="sm" variant="primary" loading={busy} disabled={!usable.ok} onClick={gen}>
            生成
          </UiButton>
          <UiButton size="sm" disabled={!generated} onClick={copyGenerated}>
            复制
          </UiButton>
          {!usable.ok ? (
            <span className="text-sk-err">{usable.reason}</span>
          ) : (
            <span className="inline-flex items-center gap-2 text-xs text-sk-sub" style={{ width: 260 }}>
              <UiProgress
                percent={pwgenMeterPercent(bits)}
                tone={tier.color === "red" ? "err" : tier.color === "orange" ? "warn" : "ok"}
              />
              强度{tier.label} · 约 {Math.round(bits)} 位熵
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

export function VaultPage() {
  const toast = useToast();
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [password, setPassword] = useState("");
  const [newMode, setNewMode] = useState<"dpapi" | "password">("password");
  const [busy, setBusy] = useState(false);
  // 安全体检（重复密码检测）：显式触发，不进首屏；明文不出后端（掩码返回）
  const [reused, setReused] = useState<{
    open: boolean;
    loading: boolean;
    groups: { masked: string; accounts: { site: string; label: string }[] }[];
    weak: { masked: string; reason: string; site: string; label: string }[];
    weakCount: number;
  }>({ open: false, loading: false, groups: [], weak: [], weakCount: 0 });
  // ---------------- 状态化模态（v1.4 UI 迁移：确认弹窗 → ui/Modal） ----------------
  // 表单模态全部受控：输入值进 React state，不再用「闭包 form + DOM 取值」
  const [siteForm, setSiteForm] = useState<{
    open: boolean; site?: SiteRow;
    name: string; domain: string; url: string; aliases: string; tags: string;
  } | null>(null);
  const [accountForm, setAccountForm] = useState<{
    open: boolean; account?: AccountRow;
    label: string; username: string; password: string; totp_secret: string; notes: string;
  } | null>(null);
  const [unlockOpen, setUnlockOpen] = useState(false);
  const [unlockPw, setUnlockPw] = useState("");
  const [unlockErr, setUnlockErr] = useState("");
  const [masterForm, setMasterForm] = useState<{ changing: boolean; current: string; next: string } | null>(null);
  const [masterErr, setMasterErr] = useState("");
  const [removeMasterOpen, setRemoveMasterOpen] = useState(false);
  const [removeMasterPw, setRemoveMasterPw] = useState("");
  const [removeMasterErr, setRemoveMasterErr] = useState("");
  // 两栏行高测量（解锁态才挂测量）：站点/账号再多只滚列表卡体
  const row = useFillRowHeight(status?.unlocked === true);
  // 条目数据（解锁后从后端读）
  const [sites, setSites] = useState<SiteRow[]>([]);
  const [sitesLoading, setSitesLoading] = useState(false);
  const [selectedSite, setSelectedSite] = useState<number | null>(null);
  const [accounts, setAccounts] = useState<AccountRow[]>([]);
  const [accountsLoading, setAccountsLoading] = useState(false);
  // 联系方式（账号级一对多）：按 account_id 索引，随账号列表**一次批量**拉回
  // —— 逐行 sk_vault_contacts 在"一个站点几十个账号"时是几十次 IPC，
  // 实测列表切换会肉眼可见地卡（后端 sk_vault_accounts_contacts 一条 SQL 拿齐）。
  const [contacts, setContacts] = useState<Record<number, ContactRow[]>>({});
  // 联系方式编辑弹窗（独立弹窗而非塞进账号表单：账号表单已有密码生成面板
  // + TOTP 导入，再叠多值编辑器会高到滚动不便）
  const [contactForm, setContactForm] = useState<{
    open: boolean;
    account?: AccountRow;
    rows: ContactRow[];
    loading: boolean;
    kind: "email" | "phone";
    value: string;
    label: string;
    error: string;
  } | null>(null);
  const [search, setSearch] = useState("");
  // 账号搜索（走 `sk_vault_site_accounts` 的 q 参数；换站时重置）
  const [accSearch, setAccSearch] = useState("");
  // 👁️ 明文临时显示：一次只亮一行、30 秒定时器强制熄灭（「临时」是硬保证，
  // 不依赖用户记得再点一次）。明文只活在渲染里，复制仍走 sk_vault_copy_secret
  // 敏感通道；get_secret 是后端为「看一眼手抄」专开的使用时解密口子。
  const [reveal, setReveal] = useState<{ id: number; text: string } | null>(null);
  const revealTimer = useRef<number | undefined>(undefined);
  const totpFileRef = useRef<HTMLInputElement | null>(null);
  // 解锁保持时长（vault_autolock_min 分钟；0=不自动锁）——锁定态显示在工具条
  const [autolockMin, setAutolockMin] = useState<number | null>(null);
  // TOTP 动态码：页内常驻卡（30s 滚动 + 倒计时 + 复制/关闭）。
  // 旧实现用 message.open 气泡：自定义内容 + 常驻 + 每轮重渲在迁移后没有对应件，
  // 且气泡会被后续 toast 顶掉——页内卡更可靠，也符合「动态码的价值是贴到别处」。
  // 复制处理器在 showTotp 内构造并存入 state：动态码的展示与复制是同一条
  // 纪律单元（源码守卫 tests/vault.test.ts 切 showTotp 区块锁定敏感通道）。
  const [totp, setTotp] = useState<{
    account: AccountRow;
    code: string;
    remain: number;
    copy: () => void;
  } | null>(null);
  const totpTimer = useRef<number | undefined>(undefined);
  const phase = vaultPhase(status);
  const trustSecs = status?.trusted_until ?? 0;
  // 右栏展示的站点：必须在当前（过滤后的）站点列表里，否则视为未选中
  const selSite = selectedSite !== null ? (sites.find((s) => s.id === selectedSite) ?? null) : null;
  const setPwCheck = canSetMasterPassword(status);

  const stopTotp = useCallback(() => {
    if (totpTimer.current !== undefined) window.clearInterval(totpTimer.current);
    totpTimer.current = undefined;
    setTotp(null);
  }, []);

  useEffect(() => {
    return () => {
      if (totpTimer.current !== undefined) window.clearInterval(totpTimer.current);
    };
  }, []);

  useEffect(() => {
    void invoke<{ items: { key: string; current: unknown }[] }>("sk_settings_all")
      .then((all) => {
        const v = all.items?.find((i) => i.key === "vault_autolock_min")?.current;
        setAutolockMin(typeof v === "number" ? v : 30);
      })
      .catch(() => setAutolockMin(30));
  }, []);

  const load = useCallback(async () => {
    try {
      setStatus(await invoke<VaultStatus>("sk_vault_status"));
    } catch (e) {
      toast.error(`读取密码库状态失败：${String(e)}`);
    }
  }, [toast]);

  useEffect(() => {
    void load();
  }, [load]);

  // ---------------- 条目数据 ----------------

  const loadSites = useCallback(
    async (q = "") => {
      setSitesLoading(true);
      setSearch(q);
      try {
        setSites(await invoke<SiteRow[]>("sk_vault_sites", { q }));
      } catch {
        // 未解锁等后端原因：不弹错（面板已表达状态），仅清空
        setSites([]);
      } finally {
        setSitesLoading(false);
      }
    },
    [],
  );

  /**
   * 批量拉联系方式（一次 IPC）。空 id 列表直接清空——后端收到空数组会返回
   * 空 map，省一次往返。
   */
  const loadContacts = useCallback(async (ids: number[]) => {
    if (ids.length === 0) {
      setContacts({});
      return;
    }
    try {
      const map = await invoke<Record<string, ContactRow[]>>("sk_vault_accounts_contacts", {
        accountIds: ids,
      });
      // JSON 对象的 key 必然是字符串，转回数字索引（取值时统一用数字键）
      const out: Record<number, ContactRow[]> = {};
      for (const [k, v] of Object.entries(map ?? {})) out[Number(k)] = v ?? [];
      setContacts(out);
    } catch {
      // 老版本后端没有该命令 / 未解锁：不阻断列表，仅不显示联系方式列内容
      setContacts({});
    }
  }, []);

  const loadAccounts = useCallback(
    async (q?: string) => {
      if (selectedSite === null) return;
      const query = q ?? accSearch;
      setAccSearch(query);
      setReveal(null); // 行集合即将刷新，旧明文展示作废
      setAccountsLoading(true);
      try {
        const rows = await invoke<AccountRow[]>("sk_vault_site_accounts", { siteId: selectedSite, q: query });
        setAccounts(rows);
        await loadContacts(rows.map((a) => a.id));
      } catch {
        setAccounts([]);
        setContacts({});
      } finally {
        setAccountsLoading(false);
      }
    },
    [selectedSite, accSearch, loadContacts],
  );

  useEffect(() => {
    // 解锁状态下首次拉站点；锁定/未建库时清空（明文展示一并熄灭）
    if (phase === "unlocked") void loadSites(search);
    else {
      setSites([]);
      setAccounts([]);
      setContacts({});
      setContactForm(null);
      setSelectedSite(null);
      setAccSearch("");
      setReveal(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase]);

  useEffect(() => {
    // 行数据即将整体刷新（换站/保存/删除），旧明文展示不再可信
    setReveal(null);
    stopTotp();
    if (selectedSite !== null) void loadAccounts("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedSite]);

  const copySecret = useCallback(async (accountId: number, field: string) => {
    const r = await invoke<{ note: string; ttl: number }>("sk_vault_copy_secret", {
      accountId,
      field,
    });
    toast.success(r.ttl > 0 ? `已复制；${r.ttl} 秒后自动清空剪贴板` : "已复制（不会进入历史）");
  }, [toast]);

  // 👁️ 掩码 ↔ 明文切换。30 秒自动熄灭：用户「看一眼手抄」是常态场景，
  // 忘了点回去也不能让明文常驻屏幕（截图/旁视风险）。换行显示即作废。
  const toggleReveal = useCallback(
    async (a: AccountRow) => {
      if (reveal?.id === a.id) {
        setReveal(null);
        return;
      }
      try {
        const text = await invoke<string>("sk_vault_get_secret", {
          accountId: a.id,
          field: "password",
        });
        if (!text) {
          toast.error("该账号没有存储密码");
          return;
        }
        setReveal({ id: a.id, text });
        if (revealTimer.current !== undefined) window.clearTimeout(revealTimer.current);
        revealTimer.current = window.setTimeout(() => setReveal(null), 30_000);
      } catch (e) {
        toast.error(String(e));
      }
    },
    [reveal, toast],
  );

  // 到期自动滚动到下一个码：TOTP 30s 一轮，静态显示会让用户抄到过期码
  // （真机走查实锤）。常驻卡同时提供「复制」——动态码的价值就是贴到别处。
  // 复制必须走敏感通道（不进本应用历史/不进 Win+V 与云剪贴板/到期清空）——
  // Python _copy_totp 走 _copy_sensitive 同纪律。此前误接明文复制通道，
  // 监听器会把动态码当普通文本记进历史（源码守卫锁死）。
  const showTotp = useCallback(
    (a: AccountRow) => {
      const copy = (c: string) => {
        void invoke<{ note: string; ttl: number }>("sk_clip_copy_sensitive", { text: c })
          .then((r) =>
            toast.success(r.ttl > 0 ? `动态码已复制；${r.ttl} 秒后自动清空` : "动态码已复制"),
          )
          .catch((e) => toast.error(String(e)));
      };
      if (totpTimer.current !== undefined) window.clearInterval(totpTimer.current);
      totpTimer.current = undefined;
      const render = () => {
        void invoke<string>("sk_vault_totp_code", { accountId: a.id })
          .then((code) => {
            if (!code) {
              stopTotp();
              toast.error("TOTP 密钥无效（含字母表外字符？）");
              return;
            }
            const remain = 30 - (Math.floor(Date.now() / 1000) % 30);
            setTotp({ account: a, code, remain, copy: () => copy(code) });
          })
          .catch((e) => {
            stopTotp();
            toast.error(String(e));
          });
      };
      render();
      totpTimer.current = window.setInterval(render, 1000);
      // 兜底清理：90s 后若用户没关，停止滚动避免无限轮询（再点「动态码」重启）
      window.setTimeout(() => {
        if (totpTimer.current !== undefined) stopTotp();
      }, 90_000);
    },
    [stopTotp, toast],
  );

  const siteModal = useCallback(
    (site?: SiteRow) => {
      setSiteForm({
        open: true, site,
        name: site?.name ?? "", domain: site?.domain ?? "", url: site?.url ?? "",
        aliases: site?.aliases ?? "", tags: site?.tags ?? "",
      });
    },
    [],
  );

  /** 站点表单保存（受控值直接来自 state；URL 失焦自动带出域名的逻辑在输入框上）。 */
  const saveSite = useCallback(
    async (f: NonNullable<typeof siteForm>) => {
      if (!f.name.trim()) {
        toast.error("名称不能为空");
        return false;
      }
      const form = { name: f.name, domain: f.domain, url: f.url, aliases: f.aliases, tags: f.tags };
      if (f.site) {
        await invoke("sk_vault_update_site", { siteId: f.site.id, ...form });
      } else {
        const id = await invoke<number>("sk_vault_add_site", { ...form });
        setSelectedSite(id);
      }
      toast.success("已保存");
      await loadSites(search);
      return true;
    },
    [search, loadSites, toast],
  );

  const addSiteModal = useCallback(() => siteModal(), [siteModal]);

  const confirmDeleteSite = useCallback(
    (r: SiteRow) => {
      confirm({
        title: `删除站点「${r.name}」`,
        content: `将同时删除其下 ${r.n_accounts} 个账号，且不可恢复。`,
        okText: "删除",
        danger: true,
        onOk: async () => {
          await invoke("sk_vault_delete_site", { siteId: r.id });
          toast.success("已删除");
          if (selectedSite === r.id) setSelectedSite(null);
          await loadSites(search);
        },
      });
    },
    [selectedSite, loadSites, search, toast],
  );

  const accountModal = useCallback(
    (account?: AccountRow) => {
      if (selectedSite === null) return;
      setAccountForm({
        open: true, account,
        label: account?.label ?? "",
        username: account?.username ?? "",
        password: "",
        totp_secret: "",
        notes: "",
      });
    },
    [selectedSite],
  );

  /** 账号表单的 TOTP 图片导入（受控 state 版，不再走 DOM 取值）。 */
  const importTotpQr = useCallback(
    async (file: File) => {
      if (!accountForm) return;
      try {
        const buf = new Uint8Array(await file.arrayBuffer());
        let bin = "";
        for (let i = 0; i < buf.length; i += 0x8000) {
          bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
        }
        const text = await invoke<string>("sk_qr_decode_image", { data: btoa(bin) });
        const info = parseOtpauth(text);
        if (!info) {
          toast.info("图中未识别到 otpauth:// 二维码");
          return;
        }
        setAccountForm((f) => (f ? { ...f, totp_secret: info.secret } : f));
        const bad = nonDefaultParams(info);
        if (bad.length) {
          toast.info(
            `该码使用非标准参数（${bad.join("、")}）——本库按默认 SHA1/6/30 生成动态码，导入后可能不匹配`,
          );
        } else {
          toast.success(`已导入 ${info.issuer || info.label || "TOTP 密钥"}`);
        }
      } catch (err) {
        toast.error(String(err));
      }
    },
    [accountForm, toast],
  );

  /** 账号表单保存（受控值来自 state）。返回 true = 成功并关闭。 */
  const saveAccount = useCallback(
    async (f: NonNullable<typeof accountForm>): Promise<boolean> => {
      if (!f.account && !f.password) {
        toast.error("密码不能为空");
        return false;
      }
      if (f.account) {
        await invoke("sk_vault_update_account", {
          accountId: f.account.id,
          label: f.label,
          username: f.username,
          password: f.password,          // 空串=不改（后端口径）
          totpSecret: f.totp_secret,
          notes: f.notes,
        });
      } else {
        await invoke("sk_vault_add_account", {
          siteId: selectedSite,
          password: f.password,
          label: f.label || f.username,   // 标签留空 = 用用户名，列表更好认
          username: f.username,
          totpSecret: f.totp_secret,
          notes: f.notes,
        });
      }
      toast.success("已保存");
      await loadAccounts();
      await loadSites(search);
      return true;
    },
    [selectedSite, loadAccounts, loadSites, search, toast],
  );

  const addAccountModal = useCallback(() => accountModal(), [accountModal]);

  // 删除账号确认（操作列「···」菜单入口）：危险操作保持显式二次确认
  const confirmDeleteAccount = useCallback(
    (a: AccountRow) => {
      confirm({
        title: `删除账号「${accountTitle(a)}」`,
        okText: "删除",
        danger: true,
        onOk: async () => {
          await invoke("sk_vault_delete_account", { accountId: a.id });
          await loadAccounts();
          await loadSites(search);
        },
      });
    },
    [loadAccounts, loadSites, search],
  );

  const toggleFav = useCallback(
    (a: AccountRow) => {
      void invoke("sk_vault_toggle_favorite", { accountId: a.id })
        .then(() => void loadAccounts())
        .catch((e) => toast.error(String(e)));
    },
    [loadAccounts, toast],
  );

  // ---------------- 账号级联系方式（邮箱 / 电话，一对多） ----------------

  /** 打开联系方式弹窗（顺带把该账号现有条目拉齐）。 */
  const contactModal = useCallback(
    async (a: AccountRow) => {
      setContactForm({
        open: true, account: a, rows: contacts[a.id] ?? [], loading: true,
        kind: "email", value: "", label: "", error: "",
      });
      try {
        const rows = await invoke<ContactRow[]>("sk_vault_contacts", { accountId: a.id });
        setContactForm((f) => (f ? { ...f, rows, loading: false } : f));
        setContacts((m) => ({ ...m, [a.id]: rows }));
      } catch (e) {
        setContactForm((f) => (f ? { ...f, loading: false, error: String(e) } : f));
      }
    },
    [contacts],
  );

  /** 新增一条（后端校验，前端预检同口径——即时反馈，避免空跑一次往返）。 */
  const addContact = useCallback(async () => {
    const f = contactForm;
    if (!f?.account) return;
    const chk = validateContact(f.kind, f.value);
    if (!chk.ok) {
      setContactForm({ ...f, error: chk.reason });
      return;
    }
    try {
      await invoke<number>("sk_vault_add_contact", {
        accountId: f.account.id,
        kind: f.kind,
        value: f.value.trim(),
        label: f.label.trim(),
        isPrimary: false, // 同类型第一条后端自动设为主项；后续追加不抢主项
      });
      const rows = await invoke<ContactRow[]>("sk_vault_contacts", { accountId: f.account.id });
      setContactForm({ ...f, rows, value: "", label: "", error: "" });
      setContacts((m) => ({ ...m, [f.account!.id]: rows }));
    } catch (e) {
      setContactForm({ ...f, error: String(e).replace(/^Error:\s*/, "") });
    }
  }, [contactForm]);

  const setPrimaryContact = useCallback(
    async (id: number) => {
      const f = contactForm;
      if (!f?.account) return;
      try {
        await invoke("sk_vault_set_primary_contact", { contactId: id });
        const rows = await invoke<ContactRow[]>("sk_vault_contacts", { accountId: f.account.id });
        setContactForm({ ...f, rows, error: "" });
        setContacts((m) => ({ ...m, [f.account!.id]: rows }));
      } catch (e) {
        setContactForm({ ...f, error: String(e) });
      }
    },
    [contactForm],
  );

  const deleteContact = useCallback(
    async (c: ContactRow) => {
      const f = contactForm;
      if (!f?.account) return;
      try {
        await invoke("sk_vault_delete_contact", { contactId: c.id });
        const rows = await invoke<ContactRow[]>("sk_vault_contacts", { accountId: f.account.id });
        setContactForm({ ...f, rows, error: "" });
        setContacts((m) => ({ ...m, [f.account!.id]: rows }));
      } catch (e) {
        setContactForm({ ...f, error: String(e) });
      }
    },
    [contactForm],
  );

  // ---------------- 解锁 / 主密码（全部收进顶部工具条） ----------------

  const setup = useCallback(() => {
    confirm({
      title: newMode === "password" ? "创建密码库（主密码模式）" : "创建密码库（免密模式）",
      width: 560,
      content: setupConfirmText(newMode),
      okText: "创建",
      onOk: async () => {
        setBusy(true);
        try {
          const r = await invoke<{ message: string }>("sk_vault_setup", {
            password: newMode === "password" ? password : null,
          });
          toast.success(r.message);
          setPassword("");
          await load();
        } catch (e) {
          toast.error(String(e));
        } finally {
          setBusy(false);
        }
      },
    });
  }, [newMode, password, load, toast]);

  const unlock = useCallback(
    async (pw: string | null) => {
      setBusy(true);
      try {
        const r = await invoke<{ message: string }>("sk_vault_unlock", { password: pw });
        toast.success(r.message);
        setPassword("");
        await load();
      } catch (e) {
        // 后端区分「密码错误」与「文件损坏」，原样展示
        toast.error(String(e));
      } finally {
        setBusy(false);
      }
    },
    [load],
  );

  // 「解锁…」：免密模式（DPAPI）直接静默解锁，对齐 Python 版语义；
  // 主密码模式弹出状态化输入框（受控 + 行内错误，AntD 时代的静态快照/
  // 非受控 DOM hack 不再需要——那个「输入和显示不一样」的根因随之消失）。
  const openUnlock = useCallback(() => {
    if (!status?.has_master_password) {
      void unlock(null);
      return;
    }
    setUnlockPw("");
    setUnlockErr("");
    setUnlockOpen(true);
  }, [status, unlock]);

  /** 解锁模态提交：错误行内显示，弹窗不关（用户可改密码重试）。 */
  const submitUnlock = useCallback(async () => {
    setBusy(true);
    setUnlockErr("");
    try {
      const r = await invoke<{ message: string }>("sk_vault_unlock", { password: unlockPw });
      toast.success(r.message);
      setUnlockOpen(false);
      setPassword("");
      await load();
    } catch (e) {
      // 后端区分「密码错误」与「文件损坏」，行内原样展示
      setUnlockErr(String(e).replace(/^Error:\s*/, ""));
    } finally {
      setBusy(false);
    }
  }, [unlockPw, load, toast]);

  const lock = useCallback(async () => {
    await invoke("sk_vault_lock");
    toast.success("已锁定");
    setPassword("");
    await load();
  }, [load, toast]);

  // 设置/更改主密码：已是主密码模式时先验当前密码（对齐 Python 版，
  // 防止信任窗口内被静默换锁），再写新密码。
  const runReuseCheck = useCallback(async () => {
    setReused((r) => ({ ...r, open: true, loading: true }));
    try {
      const d = await invoke<{
        count: number;
        groups: { masked: string; accounts: { site: string; label: string }[] }[];
        weak: { masked: string; reason: string; site: string; label: string }[];
        weak_count: number;
      }>("sk_vault_reused_passwords");
      setReused({
        open: true,
        loading: false,
        groups: d.groups,
        weak: d.weak ?? [],
        weakCount: d.weak_count ?? 0,
      });
    } catch (e) {
      setReused({ open: false, loading: false, groups: [], weak: [], weakCount: 0 });
      toast.error(String(e));
    }
  }, [toast]);

  const setMaster = useCallback(() => {
    const changing = status?.has_master_password === true;
    setMasterErr("");
    setMasterForm({ changing, current: "", next: "" });
  }, [status]);

  /** 主密码表单提交：改密前先验当前密码（信任窗口内防静默换锁）。错误行内显示。 */
  const submitMaster = useCallback(async () => {
    if (!masterForm) return;
    setMasterErr("");
    try {
      if (masterForm.changing) {
        const ok = await invoke<boolean>("sk_vault_verify_password", { password: masterForm.current });
        if (!ok) {
          setMasterErr("当前主密码错误，未做改动");
          return;
        }
      }
      toast.success(await invoke<string>("sk_vault_set_password", { password: masterForm.next }));
      setPassword("");
      setMasterForm(null);
      await load();
    } catch (e) {
      setMasterErr(String(e).replace(/^Error:\s*/, ""));
    }
  }, [masterForm, load, toast]);

  const removeMaster = useCallback(() => {
    setRemoveMasterPw("");
    setRemoveMasterErr("");
    setRemoveMasterOpen(true);
  }, []);

  /** 移除主密码提交：验证当前密码，错误行内显示、弹窗不关。 */
  const submitRemoveMaster = useCallback(async () => {
    setRemoveMasterErr("");
    try {
      const ok = await invoke<boolean>("sk_vault_remove_password", { current: removeMasterPw });
      if (!ok) {
        setRemoveMasterErr("主密码错误，未做改动");
        return;
      }
      toast.success("已移除主密码，当前为免密模式");
      setRemoveMasterOpen(false);
      setPassword("");
      await load();
    } catch (e) {
      setRemoveMasterErr(String(e).replace(/^Error:\s*/, ""));
    }
  }, [removeMasterPw, load, toast]);

  const accountColumns: Column<AccountRow>[] = [
    {
      // 账号单元格两行：标题（label 优先）+ 主联系方式。
      // 联系方式不单开一列——980 宽窗口下右栏只有约 430px，6 列必然横向滚动；
      // 而"认账号"本来就该在一处看全（谁、用哪个邮箱/电话）。
      title: "账号",
      key: "title",
      render: (_: unknown, a) => {
        const s = contactSummary(contacts[a.id] ?? []);
        const line = (kind: "email" | "phone", text: string, more: number) =>
          text ? (
            <UiTooltip title={`双击复制${contactKindLabel(kind)}`}>
              <span
                className="inline-flex max-w-full cursor-copy items-center gap-1 overflow-hidden text-ellipsis whitespace-nowrap"
                onDoubleClick={(e) => {
                  e.stopPropagation();
                  void invoke("sk_clip_copy_plain", { text })
                    .then(() => toast.success(`${contactKindLabel(kind)}已复制`))
                    .catch((err) => toast.error(String(err)));
                }}
              >
                <span style={{ opacity: 0.55 }}>{kind === "email" ? "✉" : "☎"}</span>
                <span className="overflow-hidden text-ellipsis">{text}</span>
                {more > 0 && <span className="sk-hint">+{more}</span>}
              </span>
            </UiTooltip>
          ) : null;
        return (
          <div className="flex min-w-0 flex-col gap-0.5">
            <span className="inline-flex items-center gap-1">
              {a.is_favorite && <span>⭐</span>}
              <span className="overflow-hidden text-ellipsis whitespace-nowrap">
                {accountTitle(a)}
              </span>
            </span>
            {(s.email || s.phone) && (
              <span className="flex min-w-0 flex-col text-xs" style={{ color: "var(--sk-sub)" }}>
                {line("email", s.email, s.emailMore)}
                {line("phone", s.phone, s.phoneMore)}
              </span>
            )}
          </div>
        );
      },
    },
    {
      title: "用户名",
      key: "username",
      render: (_: unknown, a) =>
        a.username ? (
          <UiTooltip title="双击复制用户名（不进剪贴板历史）">
            <span
              className="cursor-copy whitespace-nowrap"
              onDoubleClick={(e) => {
                // 行双击 = 编辑；用户名双击 = 复制——各归各，阻断冒泡
                e.stopPropagation();
                void invoke("sk_clip_copy_plain", { text: a.username })
                  .then(() => toast.success("用户名已复制"))
                  .catch((err) => toast.error(String(err)));
              }}
            >
              {a.username}
            </span>
          </UiTooltip>
        ) : (
          <span className="sk-hint">—</span>
        ),
    },
    {
      // 密码列恒定纯掩码（固定 8 圆点，长度不外泄）+ 单行 ellipsis，
      // 双击复制。显示/复制动作收进行尾 ··· 菜单——行内挂 👁️/📋
      // 两个图标太吵（用户实锤「不用这么多图标」），且窄表里
      // 图标排让列内容自成一体。
      title: "密码",
      key: "password",
      width: 150,
      render: (_: unknown, a) => {
        const revealed = reveal?.id === a.id;
        return (
          <UiTooltip
            title={
              revealed
                ? "点击 👁 隐藏；双击复制密码（敏感通道，25 秒后自动清空剪贴板）"
                : "双击复制密码（敏感通道，25 秒后自动清空剪贴板）；悬停点 👁 显示明文（30 秒后自动隐藏）"
            }
          >
            <span className="group inline-flex items-center gap-1">
              <span
                className="inline-block max-w-[110px] cursor-copy overflow-hidden align-bottom text-ellipsis whitespace-nowrap"
                style={{ letterSpacing: revealed ? 0 : 2 }}
                onDoubleClick={(e) => {
                  e.stopPropagation(); // 行双击 = 编辑，复制密码不触发编辑
                  void copySecret(a.id, "password");
                }}
              >
                {revealed ? reveal?.text : maskedSecret()}
              </span>
              {/* 悬停才浮现的 👁：显示/隐藏明文（30 秒定时强制熄灭）。
                  不做成常驻图标——用户实锤过「行内图标太多」；但 ··· 菜单
                  藏太深（用户实测找不到"密码怎么看"），悬停是折中。 */}
              <button
                type="button"
                aria-label={revealed ? "隐藏密码" : "显示密码"}
                className="shrink-0 cursor-pointer border-0 bg-transparent p-0 text-xs opacity-0 transition-opacity duration-150 group-hover:opacity-60 hover:!opacity-100"
                onClick={(e) => {
                  e.stopPropagation();
                  void toggleReveal(a);
                }}
              >
                {revealed ? "🙈" : "👁"}
              </button>
            </span>
          </UiTooltip>
        );
      },
    },
    // 「标签」列已移除：accountTitle 本来就是 label 优先，同一信息显示两遍。
    // 省下的 120px 给窄窗口下的账号/用户名列，避免横向滚动。
    {
      // 操作列收进行尾「···」菜单：置顶/编辑/删除/动态码/显示密码/
      // 复制密码全部菜单化（密码列只留纯掩码，行内不放图标——
      // 用户实锤「不用这么多图标」）。删除仍带二次确认弹窗。
      title: "操作",
      key: "ops",
      width: 64,
      render: (_: unknown, a) => {
        const revealed = reveal?.id === a.id;
        const items: MenuItem[] = [
          ...(a.has_totp ? [{ key: "totp", label: "⏱ 动态码" }] : []),
          { key: "fav", label: a.is_favorite ? "⭐ 取消置顶" : "📌 置顶" },
          { key: "edit", label: "✏️ 编辑" },
          { key: "contacts", label: "📇 联系方式" },
          {
            key: "reveal",
            label: revealed ? "🙈 隐藏密码" : "👁️ 显示密码（30 秒后自动隐藏）",
          },
          { key: "copypw", label: "📋 复制密码" },
          { key: "sep", type: "divider" as const, label: "" },
          { key: "del", danger: true, label: "🗑️ 删除" },
        ];
        return (
          <Dropdown
            trigger={
              <UiButton size="sm" aria-label="更多操作">
                ···
              </UiButton>
            }
            items={items}
            onItemClick={(key) => {
              if (key === "totp") showTotp(a);
              else if (key === "fav") toggleFav(a);
              else if (key === "edit") accountModal(a);
              else if (key === "contacts") void contactModal(a);
              else if (key === "reveal") void toggleReveal(a);
              else if (key === "copypw") void copySecret(a.id, "password");
              else if (key === "del") confirmDeleteAccount(a);
            }}
          />
        );
      },
    },
  ];

  return (
    <div
      className="flex w-full min-w-0 flex-col gap-3 pr-1"
      style={{ minHeight: "100%" }} // 两栏行 flex:1 的前提：根至少撑满 Content 视口高
    >
      {/* 顶部工具条：左侧状态徽章，右侧全部主密码操作（一排式，对齐 Python 版） */}
      <Card className="flex flex-wrap items-center gap-2 px-3 py-2">
        <div className="flex flex-wrap items-center gap-2">
          {phase === "unlocked" ? (
            // 状态徽章与自动锁定合并为倒计时标签（分钟粒度）：trusted_until
            // 有窗口时按它倒计时（后端给的是剩余秒）；无窗口时退回
            // 「闲置 autolockMin 后」口径——后端重锁按闲置计时（每次操作
            // 续期），前端没有闲置钟，不做会撒谎的假秒表。
            <UiTooltip title="解锁在此时长内免输主密码；闲置到期自动重新锁定，操作会续期">
              <Badge tone="ok">
                {trustSecs > 0
                  ? `🔓 已解锁 · ${Math.max(1, Math.ceil(trustSecs / 60))} 分钟后自动锁`
                  : autolockMin === 0
                    ? "🔓 已解锁"
                    : autolockMin === null
                      ? "🔓 已解锁 · …"
                      : `🔓 已解锁 · 闲置 ${autolockMin === 1440 ? "1 天" : `${autolockMin} 分钟`}后自动锁`}
              </Badge>
            </UiTooltip>
          ) : (
            <Badge tone={phase === "locked" ? "warn" : "neutral"}>
              {phase === "locked" ? "🔒 已锁定" : "未初始化"}
            </Badge>
          )}
          <span className="sk-hint">{modeLabel(status?.mode ?? "")}</span>
          {phase === "locked" && (
            <UiTooltip title="解锁在此时长内免输主密码；到期自动重新锁定">
              <Select
                className="w-[150px] text-xs"
                value={String(autolockMin ?? 30)}
                onChange={(v) => {
                  const n = Number(v);
                  setAutolockMin(n);
                  const keep = n === 0 ? "已设为不自动锁定" : `解锁后将保持 ${n === 1440 ? "1 天" : `${n} 分钟`}`;
                  void invoke("sk_settings_set", { key: "vault_autolock_min", value: n })
                    .then(() => toast.success(keep))
                    .catch((e) => toast.error(String(e)));
                }}
                options={[
                  { value: "30", label: "保持 30 分钟" },
                  { value: "1440", label: "保持 1 天" },
                  { value: "0", label: "不自动锁定" },
                ]}
              />
            </UiTooltip>
          )}
        </div>
        <div className="min-w-2 flex-1" />
        <div className="flex flex-wrap items-center gap-2">
          {phase === "locked" && (
            <UiButton size="sm" variant="primary" loading={busy} onClick={openUnlock}>
              解锁…
            </UiButton>
          )}
          {phase === "unlocked" && (
            <UiButton size="sm" onClick={() => void lock()}>
              锁定
            </UiButton>
          )}
          {phase === "unlocked" && setPwCheck.ok && (
            <UiButton size="sm" onClick={setMaster}>
              设置主密码…
            </UiButton>
          )}
          {phase === "unlocked" && status?.has_master_password && (
            <>
              <UiButton size="sm" onClick={setMaster}>
                更改主密码…
              </UiButton>
              {/* 移除主密码是危险操作：从常驻红按钮收进「···」，点开仍走
                  输入当前主密码的确认弹窗（后端校验，误触第一层被挡） */}
              <Dropdown
                trigger={
                  <UiButton size="sm" aria-label="更多主密码操作">
                    ···
                  </UiButton>
                }
                items={[
                  { key: "reuse-check", label: "🩺 安全体检（重复/弱密码）" },
                  { key: "sep", type: "divider", label: "" },
                  { key: "remove-master", danger: true, label: "移除主密码" },
                ]}
                onItemClick={(key) => {
                  if (key === "remove-master") removeMaster();
                  else if (key === "reuse-check") void runReuseCheck();
                }}
              />
            </>
          )}
          {phase === "unlocked" && storageAvailable(status) && (
            <UiButton size="sm" variant="primary" onClick={addSiteModal}>
              + 网站
            </UiButton>
          )}
        </div>
      </Card>

      {phase === "unprovisioned" && (
        <Card>
          <CardHeader>
            <CardTitle>创建密码库</CardTitle>
          </CardHeader>
          <CardBody>
            <div className="flex flex-col gap-2.5">
              <Radio<"dpapi" | "password">
                value={newMode}
                onChange={setNewMode}
                options={[
                  { value: "password", label: "主密码模式（推荐）" },
                  { value: "dpapi", label: "免密模式（DPAPI，绑定当前 Windows 用户）" },
                ]}
              />
              {newMode === "password" && (
                <>
                  <PasswordInput
                    className="max-w-[380px]"
                    placeholder="主密码（至少 8 位更稳）"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                  {password && (
                    <Badge
                      tone={
                        passwordStrength(password).level === "good"
                          ? "ok"
                          : passwordStrength(password).level === "fair"
                            ? "warn"
                            : "err"
                      }
                    >
                      {passwordStrength(password).hint}
                    </Badge>
                  )}
                </>
              )}
              <UiButton variant="primary" size="sm" loading={busy} onClick={setup}>
                创建密码库
              </UiButton>
            </div>
          </CardBody>
        </Card>
      )}

      {!storageAvailable(status) ? (
        // capability 门兜底（正常恒走解锁分支）：仅在后端异常时出现
        <Alert tone="warning" title="密码库存取暂不可用">
          后端数据层异常，请查看日志并重启应用。
        </Alert>
      ) : phase === "locked" ? (
        <Alert tone="info" title="密码库已锁定">
          点右上角「解锁…」：免密模式直接打开；主密码模式输入主密码验证后查看与编辑条目。
        </Alert>
      ) : phase === "unlocked" ? (
        // 行高填满 Content 视口（useFillRow 实测）：站点/账号再多只滚列表卡体，
        // 工具条与两栏框架不动；站点少时卡片仍满窗高不留底色
        <div
          ref={row.ref}
          className="flex flex-1 items-stretch gap-3 overflow-hidden"
          style={{ minHeight: 0, height: row.h }}
        >
          {/* 左栏：网站列表（固定 ~300px，搜索嵌在卡标题）；列表在卡体内自滚 */}
          <Card className="flex w-[300px] flex-none flex-col">
            <CardHeader>
              <div className="min-w-0 flex-1">
                <UiInput
                  className="h-7 w-full text-xs"
                  placeholder="搜索站点…（回车）"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void loadSites(search);
                  }}
                />
              </div>
            </CardHeader>
            <div className="min-h-0 flex-1 overflow-y-auto px-1 pb-2">
              {sitesLoading ? (
                <div className="py-4 text-center text-sm text-sk-sub">读取中…</div>
              ) : sites.length === 0 ? (
                <div className="px-2 py-4 text-center text-[13px] text-sk-sub">
                  暂无站点——点右上角「+ 网站」开始
                </div>
              ) : (
                sites.map((r) => (
                  // 行容器用 div 而不是 button：里面还要放 Dropdown（button 里
                  // 嵌 button 是非法 HTML，点击还会冒泡成「选中站点」）
                  <div
                    key={r.id}
                    role="button"
                    tabIndex={0}
                    onClick={() => setSelectedSite(r.id)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") setSelectedSite(r.id);
                    }}
                    className={
                      "group flex w-full cursor-pointer items-start gap-1 rounded-md px-2 py-1.5 text-left transition-colors " +
                      (r.id === selectedSite
                        ? "bg-sk-accent/12"
                        : "hover:bg-sk-hover")
                    }
                  >
                    <div className="min-w-0 flex-1">
                      <div className="truncate font-semibold text-sk-text">{r.name}</div>
                      <div className="sk-hint text-[11px]">
                        {r.n_accounts} 个账号{r.domain ? ` · ${r.domain}` : ""}
                      </div>
                    </div>
                    {/* 每个站点自己的操作入口：不用先选中再去右栏找 ···。
                        悬停才出现，不干扰列表整洁（删除仍带二次确认）。 */}
                    <span
                      className="shrink-0 opacity-0 transition-opacity duration-150 group-hover:opacity-100"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <Dropdown
                        trigger={
                          <UiButton variant="ghost" size="sm" aria-label="站点操作">
                            ···
                          </UiButton>
                        }
                        items={[
                          { key: "edit", label: "✏️ 编辑站点" },
                          { key: "sep", type: "divider" as const, label: "" },
                          {
                            key: "del",
                            danger: true,
                            label: `🗑️ 删除站点（波及 ${r.n_accounts} 个账号）`,
                          },
                        ]}
                        onItemClick={(key) => {
                          if (key === "edit") siteModal(r);
                          else if (key === "del") confirmDeleteSite(r);
                        }}
                      />
                    </span>
                  </div>
                ))
              )}
            </div>
          </Card>

          {/* 右栏：该站账号（flex:1；未选中站点时给引导文案）；账号多时卡体内滚 */}
          <Card className="flex min-w-0 flex-1 flex-col">
            <CardHeader>
              {selSite ? (
                // 面包屑形态「站点 › 名称（共 N 个账号）」；编辑/删除收进 ···
                // 菜单（右上角只留主按钮「+ 账号」，操作区不再拥挤）
                <span className="flex flex-wrap items-center gap-1.5">
                  <span className="sk-hint">站点</span>
                  <span className="sk-hint">›</span>
                  <span className="font-semibold">{selSite.name}</span>
                  <span className="sk-hint">（共 {selSite.n_accounts} 个账号）</span>
                  <Dropdown
                    trigger={
                      <UiButton variant="ghost" size="sm" aria-label="站点操作">
                        ···
                      </UiButton>
                    }
                    items={[
                      { key: "edit", label: "✏️ 编辑站点" },
                      { key: "sep", type: "divider", label: "" },
                      {
                        key: "del",
                        danger: true,
                        label: `🗑️ 删除站点（波及 ${selSite.n_accounts} 个账号）`,
                      },
                    ]}
                    onItemClick={(key) => {
                      if (key === "edit") siteModal(selSite);
                      else if (key === "del") confirmDeleteSite(selSite);
                    }}
                  />
                </span>
              ) : (
                <CardTitle>账号</CardTitle>
              )}
              {selSite && (
                <UiButton size="sm" variant="primary" onClick={addAccountModal}>
                  + 账号
                </UiButton>
              )}
            </CardHeader>
            <CardBody className="min-h-0 flex-1 overflow-y-auto">
              {!selSite ? (
                <div className="text-sk-sub">
                  左侧点一个网站，这里会显示它的账号；还没有站点就点右上角「+ 网站」新建。
                </div>
              ) : (
                <div className="flex flex-col gap-2">
                  <div>
                    <UiInput
                      className="h-7 w-[260px] text-xs"
                      placeholder="搜索账号：用户名 / 标签（回车）"
                      value={accSearch}
                      onChange={(e) => setAccSearch(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") void loadAccounts();
                      }}
                    />
                  </div>
                  {totp && (
                    // TOTP 动态码常驻卡：30s 一轮自动滚动（静态显示会让人抄到过期码）
                    <div className="rounded-[10px] border border-sk-accent/35 bg-sk-accent/8 px-3 py-2.5">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="text-sm text-sk-text">
                          {accountTitle(totp.account)} 动态码：
                          <code className="mx-1 rounded bg-sk-hover px-1.5 py-0.5 text-[15px] font-semibold">
                            {totp.code}
                          </code>
                          <span className="sk-hint">（{totp.remain}s 后滚动）</span>
                        </span>
                        <UiButton size="sm" onClick={totp.copy}>
                          复制
                        </UiButton>
                        <UiButton variant="ghost" size="sm" onClick={stopTotp}>
                          关闭
                        </UiButton>
                      </div>
                    </div>
                  )}
                  <Table<AccountRow>
                    rowKey={(r) => r.id}
                    loading={accountsLoading}
                    dataSource={accounts}
                    columns={accountColumns}
                    onRowDoubleClick={(a) => accountModal(a)}
                    empty={<Empty description="该站点暂无账号——点右上角「+ 账号」添加" />}
                  />
                </div>
              )}
            </CardBody>
          </Card>
        </div>
      ) : null}

      {/* 站点表单（受控；URL 失焦自动带出域名） */}
      <UiModal
        open={siteForm?.open === true}
        title={siteForm?.site ? `编辑站点「${siteForm.site.name}」` : "添加站点"}
        onClose={() => setSiteForm(null)}
        onEnter={() => {
          if (siteForm) void saveSite(siteForm).then((ok) => ok && setSiteForm(null));
        }}
        width={520}
        footer={
          <>
            <UiButton onClick={() => setSiteForm(null)}>取消</UiButton>
            <UiButton
              variant="primary"
              onClick={() =>
                siteForm && void saveSite(siteForm).then((ok) => ok && setSiteForm(null))
              }
            >
              保存
            </UiButton>
          </>
        }
      >
        {siteForm && (
          <div className="flex flex-col gap-2">
            <UiInput
              placeholder="名称 *"
              value={siteForm.name}
              onChange={(e) => setSiteForm({ ...siteForm, name: e.target.value })}
            />
            <UiInput
              placeholder="域名（如 github.com）"
              value={siteForm.domain}
              onChange={(e) => setSiteForm({ ...siteForm, domain: e.target.value })}
            />
            <UiInput
              placeholder="URL（填后自动带出域名）"
              value={siteForm.url}
              onChange={(e) => setSiteForm({ ...siteForm, url: e.target.value })}
              onBlur={() => {
                const u = siteForm.url.trim();
                if (!u || siteForm.domain) return;
                try {
                  const host = new URL(u.includes("://") ? u : `https://${u}`).hostname;
                  if (host) setSiteForm({ ...siteForm, domain: host });
                } catch { /* 非 URL 文本：留空手填 */ }
              }}
            />
            <UiInput
              placeholder="别名（逗号分隔）"
              value={siteForm.aliases}
              onChange={(e) => setSiteForm({ ...siteForm, aliases: e.target.value })}
            />
            <UiInput
              placeholder="标签（逗号分隔）"
              value={siteForm.tags}
              onChange={(e) => setSiteForm({ ...siteForm, tags: e.target.value })}
            />
          </div>
        )}
      </UiModal>

      {/* 账号表单（受控；内嵌密码生成面板 + TOTP 扫码导入） */}
      <UiModal
        open={accountForm?.open === true}
        title={accountForm?.account ? `编辑账号「${accountTitle(accountForm.account)}」` : "添加账号"}
        onClose={() => setAccountForm(null)}
        onEnter={() => {
          if (accountForm) {
            void saveAccount(accountForm).then((ok) => ok && setAccountForm(null));
          }
        }}
        width={520}
        footer={
          <>
            <UiButton onClick={() => setAccountForm(null)}>取消</UiButton>
            <UiButton
              variant="primary"
              onClick={() =>
                accountForm &&
                void saveAccount(accountForm).then((ok) => ok && setAccountForm(null))
              }
            >
              保存
            </UiButton>
          </>
        }
      >
        {accountForm && (
          <div className="flex flex-col gap-2">
            <UiInput
              placeholder="标签（如 主账号）"
              value={accountForm.label}
              onChange={(e) => setAccountForm({ ...accountForm, label: e.target.value })}
            />
            <UiInput
              placeholder="用户名"
              value={accountForm.username}
              onChange={(e) => setAccountForm({ ...accountForm, username: e.target.value })}
            />
            <PasswordInput
              placeholder={accountForm.account ? "留空 = 不修改密码" : "密码 *"}
              value={accountForm.password}
              onChange={(e) => setAccountForm({ ...accountForm, password: e.target.value })}
            />
            <PasswordGenPanel
              onGenerated={(pw) => setAccountForm((f) => (f ? { ...f, password: pw } : f))}
            />
            <div className="flex w-full">
              <UiInput
                className="flex-1 rounded-r-none"
                placeholder="TOTP 密钥（Base32，可留空）"
                value={accountForm.totp_secret}
                onChange={(e) => setAccountForm({ ...accountForm, totp_secret: e.target.value })}
              />
              <UiButton className="rounded-l-none border-l-0" onClick={() => totpFileRef.current?.click()}>
                从图片导入
              </UiButton>
              <input
                ref={totpFileRef}
                type="file"
                accept="image/png,image/jpeg,image/webp,image/bmp"
                className="hidden"
                onChange={(e) => {
                  const f = e.target.files?.[0];
                  e.target.value = "";
                  if (f) void importTotpQr(f);
                }}
              />
            </div>
            <UiTextarea
              placeholder="备注（加密存储）"
              value={accountForm.notes}
              onChange={(e) => setAccountForm({ ...accountForm, notes: e.target.value })}
            />
          </div>
        )}
      </UiModal>

      {/* 联系方式（一个账号可有多个邮箱 / 多个电话；主项用于列表展示与默认复制） */}
      <UiModal
        open={contactForm?.open === true}
        title={contactForm?.account ? `联系方式 · ${accountTitle(contactForm.account)}` : "联系方式"}
        onClose={() => setContactForm(null)}
        width={560}
        footer={
          <>
            <UiButton onClick={() => setContactForm(null)}>关闭</UiButton>
            <UiButton
              variant="primary"
              onClick={() => void addContact()}
              disabled={!contactForm?.value.trim()}
            >
              添加
            </UiButton>
          </>
        }
      >
        {contactForm && (
          <div className="flex flex-col gap-2">
            <div className="text-xs text-sk-sub">
              同一个账号可以有多个邮箱和多个电话（换绑前的旧号、备用号都留着）。
              每类里的<b>主项</b>是列表展示与双击复制默认取用的那一个。
            </div>

            {/* 新增行：类型 + 值 + 备注标签 */}
            <div className="flex items-center gap-2">
              <Select
                className="w-24"
                aria-label="联系方式类型"
                value={contactForm.kind}
                onChange={(v) =>
                  setContactForm({ ...contactForm, kind: v === "phone" ? "phone" : "email" })
                }
                options={CONTACT_KINDS.map((k) => ({ label: k.label, value: k.value }))}
              />
              <UiInput
                className="flex-1"
                placeholder={contactForm.kind === "email" ? "邮箱（如 me@example.com）" : "电话（如 13800138000）"}
                value={contactForm.value}
                onChange={(e) => setContactForm({ ...contactForm, value: e.target.value, error: "" })}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && contactForm.value.trim()) void addContact();
                }}
              />
              <UiInput
                className="w-28"
                placeholder="备注（如 常用）"
                value={contactForm.label}
                onChange={(e) => setContactForm({ ...contactForm, label: e.target.value })}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && contactForm.value.trim()) void addContact();
                }}
              />
            </div>
            {contactForm.error && <div className="text-xs text-sk-err">{contactForm.error}</div>}

            {/* 已有条目：主项星标 / 值 / 备注 / 设为主 / 删除 */}
            <div className="mt-1 flex flex-col gap-1">
              {contactForm.loading && <div className="text-xs text-sk-sub">读取中…</div>}
              {!contactForm.loading && contactForm.rows.length === 0 && (
                <div className="text-xs text-sk-sub">还没有联系方式——上面填一条再点「添加」。</div>
              )}
              {contactForm.rows.map((c) => (
                <div
                  key={c.id}
                  className="flex items-center gap-2 rounded-[8px] border border-sk-border px-2 py-1"
                >
                  <span className="w-10 shrink-0 text-xs text-sk-sub">{contactKindLabel(c.kind)}</span>
                  <UiTooltip title="双击复制（不进剪贴板历史）">
                    <span
                      className="flex-1 cursor-copy overflow-hidden text-ellipsis whitespace-nowrap text-sm"
                      onDoubleClick={() =>
                        void invoke("sk_clip_copy_plain", { text: c.value })
                          .then(() => toast.success(`${contactKindLabel(c.kind)}已复制`))
                          .catch((e) => toast.error(String(e)))
                      }
                    >
                      {c.value}
                    </span>
                  </UiTooltip>
                  {c.label && <span className="shrink-0 text-xs text-sk-sub">{c.label}</span>}
                  {c.is_primary ? (
                    <span className="shrink-0 text-xs" style={{ color: "var(--sk-accent)" }}>
                      ★ 主项
                    </span>
                  ) : (
                    <UiButton size="sm" onClick={() => void setPrimaryContact(c.id)}>
                      设为主项
                    </UiButton>
                  )}
                  <UiButton size="sm" onClick={() => void deleteContact(c)} aria-label="删除">
                    ✕
                  </UiButton>
                </div>
              ))}
            </div>
          </div>
        )}
      </UiModal>

      {/* 解锁（受控密码框 + 行内错误；回车提交） */}
      <UiModal
        open={unlockOpen}
        title="解锁密码库"
        onClose={() => setUnlockOpen(false)}
        width={440}
        footer={
          <>
            <UiButton onClick={() => setUnlockOpen(false)}>取消</UiButton>
            <UiButton variant="primary" loading={busy} onClick={() => void submitUnlock()}>
              解锁
            </UiButton>
          </>
        }
      >
        <PasswordInput
          autoFocus
          placeholder="主密码"
          value={unlockPw}
          onChange={(e) => setUnlockPw(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !busy) void submitUnlock();
          }}
        />
        {unlockErr && <div className="mt-2 text-xs text-sk-err">{unlockErr}</div>}
      </UiModal>

      {/* 设置/更改主密码（改密前先验当前密码） */}
      <UiModal
        open={masterForm !== null}
        title={masterForm?.changing ? "更改主密码" : "设置主密码"}
        onClose={() => setMasterForm(null)}
        width={520}
        footer={
          <>
            <UiButton onClick={() => setMasterForm(null)}>取消</UiButton>
            <UiButton variant="primary" loading={busy} onClick={() => void submitMaster()}>
              {masterForm?.changing ? "更改" : "设置"}
            </UiButton>
          </>
        }
      >
        {masterForm && (
          <div className="flex flex-col gap-2">
            <div className="whitespace-pre-wrap">{setupConfirmText("password")}</div>
            {masterForm.changing && (
              <PasswordInput
                placeholder="当前主密码"
                value={masterForm.current}
                onChange={(e) => setMasterForm({ ...masterForm, current: e.target.value })}
              />
            )}
            <PasswordInput
              placeholder="新主密码（至少 8 位更稳）"
              value={masterForm.next}
              onChange={(e) => setMasterForm({ ...masterForm, next: e.target.value })}
            />
            {masterErr && <div className="mt-1 text-xs text-sk-err">{masterErr}</div>}
          </div>
        )}
      </UiModal>

      {/* 移除主密码（危险；验证当前密码） */}
      <UiModal
        open={removeMasterOpen}
        title="移除主密码"
        onClose={() => setRemoveMasterOpen(false)}
        width={480}
        footer={
          <>
            <UiButton onClick={() => setRemoveMasterOpen(false)}>取消</UiButton>
            <UiButton variant="danger" loading={busy} onClick={() => void submitRemoveMaster()}>
              移除
            </UiButton>
          </>
        }
      >
        <div className="flex flex-col gap-2">
          <div className="whitespace-pre-wrap text-sk-sub">
            移除后回到免密模式：数据密钥改由 Windows DPAPI 保护，绑定当前 Windows
            用户。任何人用你的 Windows 账户都能打开密码库。
          </div>
          <PasswordInput
            placeholder="当前主密码"
            value={removeMasterPw}
            onChange={(e) => setRemoveMasterPw(e.target.value)}
          />
          {removeMasterErr && <div className="mt-1 text-xs text-sk-err">{removeMasterErr}</div>}
        </div>
      </UiModal>

      {/* 安全体检结果：重复密码组（掩码展示，明文不出后端） */}
      <UiModal
        open={reused.open}
        title="安全体检"
        onClose={() => setReused((r) => ({ ...r, open: false }))}
        width={560}
        footer={
          <>
            <UiButton
              onClick={() => {
                // 文本报告（掩码口径——与弹窗展示同信息量，无明文泄漏面）
                const lines: string[] = ["SysKit 密码库安全体检"];
                lines.push(
                  reused.groups.length
                    ? `重复密码 ${reused.groups.length} 组（一处泄露波及全部账号）：`
                    : "重复密码：无",
                );
                reused.groups.forEach((g) => {
                  lines.push(`  ${g.masked}：`);
                  g.accounts.forEach((a) =>
                    lines.push(`    - ${a.site}${a.label ? ` · ${a.label}` : ""}`),
                  );
                });
                lines.push(
                  reused.weakCount
                    ? `弱密码 ${reused.weakCount} 个（建议改长、混用字符种类）：`
                    : "弱密码：无",
                );
                reused.weak.forEach((w) =>
                  lines.push(
                    `  ${w.masked}（${w.reason}）：${w.site}${w.label ? ` · ${w.label}` : ""}`,
                  ),
                );
                navigator.clipboard
                  .writeText(lines.join("\n"))
                  .then(() => toast.success("体检报告已复制（掩码口径，无明文）"))
                  .catch(() => toast.error("复制失败：剪贴板不可用"));
              }}
            >
              复制报告
            </UiButton>
            <UiButton onClick={() => setReused((r) => ({ ...r, open: false }))}>关闭</UiButton>
          </>
        }
      >
        {reused.loading ? (
          <div className="text-sk-sub">正在解密比对全部账号…</div>
        ) : (
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-2.5">
              <div className={reused.groups.length ? "text-sk-warn" : "text-sk-ok"}>
                {reused.groups.length
                  ? `重复密码 ${reused.groups.length} 组——一处泄露波及全部账号`
                  : "✅ 无重复密码"}
              </div>
              {reused.groups.map((g) => (
                <div key={g.masked}>
                  <code className="rounded bg-sk-hover px-1.5 py-0.5 font-mono text-xs">{g.masked}</code>
                  <ul className="mb-0 mt-1 list-disc pl-5">
                    {g.accounts.map((a, i) => (
                      <li key={i} className="sk-hint">
                        {a.site}
                        {a.label ? ` · ${a.label}` : ""}
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
            </div>
            <div className="flex flex-col gap-2.5 border-t border-sk-border/60 pt-3">
              <div className={reused.weakCount ? "text-sk-warn" : "text-sk-ok"}>
                {reused.weakCount
                  ? `弱密码 ${reused.weakCount} 个——建议改长、混用字符种类`
                  : "✅ 无弱密码"}
              </div>
              {reused.weak.map((w, i) => (
                <div key={i} className="flex items-center gap-2">
                  <code className="rounded bg-sk-hover px-1.5 py-0.5 font-mono text-xs">{w.masked}</code>
                  <span className="sk-hint">
                    {w.site}
                    {w.label ? ` · ${w.label}` : ""} · {w.reason}
                  </span>
                </div>
              ))}
            </div>
          </div>
        )}
      </UiModal>
    </div>
  );
}
