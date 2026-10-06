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
 * 纯掩码 + 👁️ 临时显示/📋 复制小图标，行操作收进 ··· 菜单）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Card,
  Dropdown,
  Input,
  Modal,
  Radio,
  Select,
  Space,
  Table,
  Tag,
  Tooltip,
  Typography,
  message,
  type MenuProps,
} from "antd";

import {
  accountTitle,
  canSetMasterPassword,
  maskedSecret,
  modeLabel,
  passwordStrength,
  setupConfirmText,
  storageAvailable,
  vaultPhase,
  type AccountRow,
  type SiteRow,
  type VaultStatus,
} from "./vault";

export function VaultPage() {
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [password, setPassword] = useState("");
  const [newMode, setNewMode] = useState<"dpapi" | "password">("password");
  const [busy, setBusy] = useState(false);
  // 条目数据（解锁后从后端读）
  const [sites, setSites] = useState<SiteRow[]>([]);
  const [sitesLoading, setSitesLoading] = useState(false);
  const [selectedSite, setSelectedSite] = useState<number | null>(null);
  const [accounts, setAccounts] = useState<AccountRow[]>([]);
  const [accountsLoading, setAccountsLoading] = useState(false);
  const [search, setSearch] = useState("");
  // 账号搜索（走 `sk_vault_site_accounts` 的 q 参数；换站时重置）
  const [accSearch, setAccSearch] = useState("");
  // 👁️ 明文临时显示：一次只亮一行、30 秒定时器强制熄灭（「临时」是硬保证，
  // 不依赖用户记得再点一次）。明文只活在渲染里，复制仍走 sk_vault_copy_secret
  // 敏感通道；get_secret 是后端为「看一眼手抄」专开的使用时解密口子。
  const [reveal, setReveal] = useState<{ id: number; text: string } | null>(null);
  const revealTimer = useRef<number | undefined>(undefined);
  // 解锁保持时长（vault_autolock_min 分钟；0=不自动锁）——锁定态显示在工具条
  const [autolockMin, setAutolockMin] = useState<number | null>(null);
  const phase = vaultPhase(status);
  const trustSecs = status?.trusted_until ?? 0;
  // 右栏展示的站点：必须在当前（过滤后的）站点列表里，否则视为未选中
  const selSite = selectedSite !== null ? (sites.find((s) => s.id === selectedSite) ?? null) : null;
  const setPwCheck = canSetMasterPassword(status);

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
      message.error(`读取密码库状态失败：${String(e)}`);
    }
  }, []);

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
      } catch (e) {
        // 未解锁等后端原因：不弹错（面板已表达状态），仅清空
        setSites([]);
      } finally {
        setSitesLoading(false);
      }
    },
    [],
  );

  const loadAccounts = useCallback(
    async (q?: string) => {
      if (selectedSite === null) return;
      const query = q ?? accSearch;
      setAccSearch(query);
      setReveal(null); // 行集合即将刷新，旧明文展示作废
      setAccountsLoading(true);
      try {
        setAccounts(await invoke<AccountRow[]>("sk_vault_site_accounts", { siteId: selectedSite, q: query }));
      } catch {
        setAccounts([]);
      } finally {
        setAccountsLoading(false);
      }
    },
    [selectedSite, accSearch],
  );

  useEffect(() => {
    // 解锁状态下首次拉站点；锁定/未建库时清空（明文展示一并熄灭）
    if (phase === "unlocked") void loadSites(search);
    else {
      setSites([]);
      setAccounts([]);
      setSelectedSite(null);
      setAccSearch("");
      setReveal(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase]);

  useEffect(() => {
    // 行数据即将整体刷新（换站/保存/删除），旧明文展示不再可信
    setReveal(null);
    if (selectedSite !== null) void loadAccounts("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedSite]);

  const copySecret = useCallback(async (accountId: number, field: string) => {
    const r = await invoke<{ note: string; ttl: number }>("sk_vault_copy_secret", {
      accountId,
      field,
    });
    message.success(r.ttl > 0 ? `已复制；${r.ttl} 秒后自动清空剪贴板` : "已复制（不会进入历史）");
  }, []);

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
          message.error("该账号没有存储密码");
          return;
        }
        setReveal({ id: a.id, text });
        if (revealTimer.current !== undefined) window.clearTimeout(revealTimer.current);
        revealTimer.current = window.setTimeout(() => setReveal(null), 30_000);
      } catch (e) {
        message.error(String(e));
      }
    },
    [reveal],
  );

  const showTotp = useCallback(
    (a: AccountRow) => {
      // 到期自动滚动到下一个码：TOTP 30s 一轮，静态显示会让用户抄到过期码
      // （真机走查实锤）。同时提供「复制」按钮——动态码的价值就是贴到别处。
      let info: ReturnType<typeof message.open> | null = message.open({
        type: "loading",
        content: "正在生成动态码…",
        duration: 0,
      });
      let timer: number | undefined;
      const stop = () => {
        if (timer !== undefined) window.clearInterval(timer);
        timer = undefined;
        info?.();
        info = null;
      };
      const render = () => {
        void invoke<string>("sk_vault_totp_code", { accountId: a.id })
          .then((code) => {
            if (!code) {
              stop();
              message.error("TOTP 密钥无效（含字母表外字符？）");
              return;
            }
            const remain = 30 - (Math.floor(Date.now() / 1000) % 30);
            info?.();
            info = message.open({
              key: `totp-${a.id}`,
              type: "info",
              duration: 0,
              content: (
                <Space size={8}>
                  <span>
                    {accountTitle(a)} 动态码：
                    <Typography.Text code style={{ fontSize: 15, fontWeight: 600 }}>
                      {code}
                    </Typography.Text>
                    <span className="sk-hint">（{remain}s 后滚动）</span>
                  </span>
                  <Button
                    size="small"
                    onClick={() => {
                      // 动态码是一次性凭证，必须走敏感通道（不进本应用历史/
                      // 不进 Win+V 与云剪贴板/到期清空）——Python _copy_totp
                      // 走 _copy_sensitive 同纪律。此前误接明文复制通道，
                      // 监听器会把动态码当普通文本记进历史（源码守卫锁死：
                      // tests/vault.test.ts「TOTP 复制走敏感通道」）。
                      void invoke<{ note: string; ttl: number }>("sk_clip_copy_sensitive", {
                        text: code,
                      })
                        .then((r) =>
                          message.success(
                            r.ttl > 0 ? `动态码已复制；${r.ttl} 秒后自动清空` : "动态码已复制",
                          ),
                        )
                        .catch((e) => message.error(String(e)));
                    }}
                  >
                    复制
                  </Button>
                </Space>
              ),
            });
            if (timer === undefined) {
              timer = window.setInterval(() => render(), Math.max(remain, 1) * 1000);
            }
          })
          .catch((e) => {
            stop();
            message.error(String(e));
          });
      };
      render();
      // 兜底清理：90s 后若用户没关，停止滚动避免无限轮询（再点「动态码」重启）
      window.setTimeout(stop, 90_000);
    },
    [],
  );

  const siteModal = useCallback(
    (site?: SiteRow) => {
      let form = { name: site?.name ?? "", domain: site?.domain ?? "", url: site?.url ?? "",
                   aliases: site?.aliases ?? "", tags: site?.tags ?? "" };
      Modal.confirm({
        title: site ? `编辑站点「${site.name}」` : "添加站点",
        width: 520,
        content: (
          <Space direction="vertical" size={8} style={{ width: "100%" }}>
            <Input id="vf-site-name" defaultValue={form.name} placeholder="名称 *"
                   onChange={(e) => (form.name = e.target.value)} />
            <Input id="vf-site-domain" defaultValue={form.domain} placeholder="域名（如 github.com）"
                   onChange={(e) => (form.domain = e.target.value)} />
            <Input id="vf-site-url" defaultValue={form.url} placeholder="URL（填后自动带出域名）"
                   onChange={(e) => (form.url = e.target.value)}
                   onBlur={() => {
                     const u = form.url.trim();
                     if (!u || form.domain) return;
                     try {
                       const host = new URL(u.includes("://") ? u : `https://${u}`).hostname;
                       if (host) {
                         form.domain = host;
                         const el = document.getElementById("vf-site-domain") as HTMLInputElement | null;
                         if (el && !el.value) el.value = host;
                       }
                     } catch { /* 非 URL 文本：留空手填 */ }
                   }} />
            <Input id="vf-site-aliases" defaultValue={form.aliases} placeholder="别名（逗号分隔）"
                   onChange={(e) => (form.aliases = e.target.value)} />
            <Input id="vf-site-tags" defaultValue={form.tags} placeholder="标签（逗号分隔）"
                   onChange={(e) => (form.tags = e.target.value)} />
          </Space>
        ),
        okText: "保存",
        cancelText: "取消",
        onOk: async () => {
          if (!form.name.trim()) {
            message.error("名称不能为空");
            throw new Error("empty name");
          }
          if (site) {
            await invoke("sk_vault_update_site", { siteId: site.id, ...form });
          } else {
            const id = await invoke<number>("sk_vault_add_site", { ...form });
            setSelectedSite(id);
          }
          message.success("已保存");
          await loadSites(search);
        },
      });
    },
    [search, loadSites],
  );

  const addSiteModal = useCallback(() => siteModal(), [siteModal]);

  const confirmDeleteSite = useCallback(
    (r: SiteRow) => {
      Modal.confirm({
        title: `删除站点「${r.name}」`,
        content: `将同时删除其下 ${r.n_accounts} 个账号，且不可恢复。`,
        okText: "删除",
        okButtonProps: { danger: true },
        cancelText: "取消",
        onOk: async () => {
          await invoke("sk_vault_delete_site", { siteId: r.id });
          message.success("已删除");
          if (selectedSite === r.id) setSelectedSite(null);
          await loadSites(search);
        },
      });
    },
    [selectedSite, loadSites, search],
  );

  const accountModal = useCallback(
    (account?: AccountRow) => {
      if (selectedSite === null) return;
      let form = {
        label: account?.label ?? "",
        username: account?.username ?? "",
        password: "",
        totp_secret: "",
        notes: "",
      };
      Modal.confirm({
        title: account ? `编辑账号「${accountTitle(account)}」` : "添加账号",
        width: 520,
        content: (
          <Space direction="vertical" size={8} style={{ width: "100%" }}>
            <Input id="vf-acc-label" defaultValue={form.label} placeholder="标签（如 主账号）"
                   onChange={(e) => (form.label = e.target.value)} />
            <Input id="vf-acc-username" defaultValue={form.username} placeholder="用户名"
                   onChange={(e) => (form.username = e.target.value)} />
            <Input.Password
              id="vf-acc-password"
              placeholder={account ? "留空 = 不修改密码" : "密码 *"}
              onChange={(e) => (form.password = e.target.value)}
            />
            <Input id="vf-acc-totp" defaultValue={form.totp_secret} placeholder="TOTP 密钥（Base32，可留空）"
                   onChange={(e) => (form.totp_secret = e.target.value)} />
            <Input.TextArea id="vf-acc-notes" defaultValue={form.notes} placeholder="备注（加密存储）"
                            onChange={(e) => (form.notes = e.target.value)} />
          </Space>
        ),
        okText: "保存",
        cancelText: "取消",
        onOk: async () => {
          if (!account && !form.password) {
            message.error("密码不能为空");
            throw new Error("empty password");
          }
          if (account) {
            await invoke("sk_vault_update_account", {
              accountId: account.id,
              label: form.label,
              username: form.username,
              password: form.password,          // 空串=不改（后端口径）
              totpSecret: form.totp_secret,
              notes: form.notes,
            });
          } else {
            await invoke("sk_vault_add_account", {
              siteId: selectedSite,
              password: form.password,
              label: form.label || form.username,   // 标签留空 = 用用户名，列表更好认
              username: form.username,
              totpSecret: form.totp_secret,
              notes: form.notes,
            });
          }
          message.success("已保存");
          await loadAccounts();
          await loadSites(search);
        },
      });
    },
    [selectedSite, loadAccounts, loadSites, search],
  );

  const addAccountModal = useCallback(() => accountModal(), [accountModal]);

  // 删除账号确认（操作列「···」菜单入口）：危险操作保持显式二次确认
  const confirmDeleteAccount = useCallback(
    (a: AccountRow) => {
      Modal.confirm({
        title: `删除账号「${accountTitle(a)}」`,
        okText: "删除",
        okButtonProps: { danger: true },
        cancelText: "取消",
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
        .catch((e) => message.error(String(e)));
    },
    [loadAccounts],
  );

  // ---------------- 解锁 / 主密码（全部收进顶部工具条） ----------------

  const setup = useCallback(() => {
    Modal.confirm({
      title: newMode === "password" ? "创建密码库（主密码模式）" : "创建密码库（免密模式）",
      width: 560,
      content: (
        <Typography.Paragraph style={{ whiteSpace: "pre-wrap" }}>
          {setupConfirmText(newMode)}
        </Typography.Paragraph>
      ),
      okText: "创建",
      cancelText: "取消",
      onOk: async () => {
        setBusy(true);
        try {
          const r = await invoke<{ message: string }>("sk_vault_setup", {
            password: newMode === "password" ? password : null,
          });
          message.success(r.message);
          setPassword("");
          await load();
        } catch (e) {
          message.error(String(e));
        } finally {
          setBusy(false);
        }
      },
    });
  }, [newMode, password, load]);

  const unlock = useCallback(
    async (pw: string | null) => {
      setBusy(true);
      try {
        const r = await invoke<{ message: string }>("sk_vault_unlock", { password: pw });
        message.success(r.message);
        setPassword("");
        await load();
      } catch (e) {
        // 后端区分「密码错误」与「文件损坏」，原样展示
        message.error(String(e));
      } finally {
        setBusy(false);
      }
    },
    [load],
  );

  // 「解锁…」：免密模式（DPAPI）直接静默解锁，对齐 Python 版语义；
  // 主密码模式弹出输入框验证。
  const openUnlock = useCallback(() => {
    if (!status?.has_master_password) {
      void unlock(null);
      return;
    }
    // Modal.confirm 的 content 是打开瞬间的静态快照：受控组件在弹窗内不
    // 重渲染（输入显示不更新、闭包值不更新）——必须用非受控捕获。
    // 这是「输入和显示不一样 + 不能解密」的根因（真机实锤 2026-10-05）。
    let input = "";
    Modal.confirm({
      title: "解锁密码库",
      width: 440,
      content: (
        <Input.Password
          autoFocus
          placeholder="主密码"
          onChange={(e) => (input = e.target.value)}
          onPressEnter={() => {
            // 回车 = 确定：触发确认框的 OK 按钮
            const ok = document.querySelector<HTMLButtonElement>(
              ".ant-modal-confirm-btns .ant-btn-primary",
            );
            ok?.click();
          }}
        />
      ),
      okText: "解锁",
      cancelText: "取消",
      onOk: () => unlock(input),
    });
  }, [status, unlock]);

  const lock = useCallback(async () => {
    await invoke("sk_vault_lock");
    message.success("已锁定");
    setPassword("");
    await load();
  }, [load]);

  // 设置/更改主密码：已是主密码模式时先验当前密码（对齐 Python 版，
  // 防止信任窗口内被静默换锁），再写新密码。
  const setMaster = useCallback(() => {
    const changing = status?.has_master_password === true;
    let current = "";
    let next = "";
    Modal.confirm({
      title: changing ? "更改主密码" : "设置主密码",
      width: 520,
      content: (
        <Space direction="vertical" size={8} style={{ width: "100%" }}>
          <Typography.Paragraph style={{ whiteSpace: "pre-wrap", marginBottom: 0 }}>
            {setupConfirmText("password")}
          </Typography.Paragraph>
          {changing && (
            <Input.Password
              id="vf-master-current"
              placeholder="当前主密码"
              onChange={(e) => (current = e.target.value)}
            />
          )}
          <Input.Password
            placeholder="新主密码（至少 8 位更稳）"
            onChange={(e) => (next = e.target.value)}
          />
        </Space>
      ),
      okText: changing ? "更改" : "设置",
      cancelText: "取消",
      onOk: async () => {
        try {
          if (changing) {
            const ok = await invoke<boolean>("sk_vault_verify_password", { password: current });
            if (!ok) {
              message.error("当前主密码错误，未做改动");
              throw new Error("wrong password");
            }
          }
          message.success(await invoke<string>("sk_vault_set_password", { password: next }));
          setPassword("");
          await load();
        } catch (e) {
          if (String(e) !== "Error: wrong password") message.error(String(e));
          throw e; // 保持对话框打开，用户可修正
        }
      },
    });
  }, [status, load]);

  const removeMaster = useCallback(() => {
    let current = "";
    Modal.confirm({
      title: "移除主密码",
      content: (
        <Space direction="vertical" size={8} style={{ width: "100%" }}>
          <Typography.Paragraph style={{ whiteSpace: "pre-wrap", marginBottom: 0 }}>
            移除后回到**免密模式**：数据密钥改由 Windows DPAPI 保护，绑定当前 Windows
            用户。任何人用你的 Windows 账户都能打开密码库。
          </Typography.Paragraph>
          <Input.Password
            placeholder="当前主密码"
            onChange={(e) => (current = e.target.value)}
          />
        </Space>
      ),
      okText: "移除",
      okButtonProps: { danger: true },
      cancelText: "取消",
      onOk: async () => {
        try {
          const ok = await invoke<boolean>("sk_vault_remove_password", { current });
          if (!ok) {
            message.error("主密码错误，未做改动");
            throw new Error("wrong password");
          }
          message.success("已移除主密码，当前为免密模式");
          setPassword("");
          await load();
        } catch (e) {
          if (String(e) !== "Error: wrong password") message.error(String(e));
          throw e;
        }
      },
    });
  }, [password, load]);

  return (
    <Space direction="vertical" size={12} style={{ width: "100%" }}>
      {/* 顶部工具条：左侧状态徽章，右侧全部主密码操作（一排式，对齐 Python 版） */}
      <Card size="small" className="sk-card" styles={{ body: { padding: "8px 12px" } }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
          <Space size={8} wrap>
            {phase === "unlocked" ? (
              // 状态徽章与自动锁定合并为倒计时标签（分钟粒度）：trusted_until
              // 有窗口时按它倒计时（后端给的是剩余秒）；无窗口时退回
              // 「闲置 autolockMin 后」口径——后端重锁按闲置计时（每次操作
              // 续期），前端没有闲置钟，不做会撒谎的假秒表。
              <Tooltip title="解锁在此时长内免输主密码；闲置到期自动重新锁定，操作会续期">
                <Tag color="green" style={{ marginInlineEnd: 0 }}>
                  {trustSecs > 0
                    ? `🔓 已解锁 · ${Math.max(1, Math.ceil(trustSecs / 60))} 分钟后自动锁`
                    : autolockMin === 0
                      ? "🔓 已解锁"
                      : autolockMin === null
                        ? "🔓 已解锁 · …"
                        : `🔓 已解锁 · 闲置 ${autolockMin === 1440 ? "1 天" : `${autolockMin} 分钟`}后自动锁`}
                </Tag>
              </Tooltip>
            ) : (
              <Tag
                color={phase === "locked" ? "orange" : "default"}
                style={{ marginInlineEnd: 0 }}
              >
                {phase === "locked" ? "🔒 已锁定" : "未初始化"}
              </Tag>
            )}
            <span className="sk-hint">{modeLabel(status?.mode ?? "")}</span>
            {phase === "locked" && (
              <Tooltip title="解锁在此时长内免输主密码；到期自动重新锁定">
                <Select
                  size="small"
                  style={{ width: 150 }}
                  value={autolockMin ?? 30}
                  onChange={(v) => {
                    setAutolockMin(v);
                    void invoke("sk_settings_set", { key: "vault_autolock_min", value: v })
                      .then(() => message.success(v === 0 ? "已设为不自动锁定" : `解锁后将保持 ${v === 1440 ? "1 天" : `${v} 分钟`}`))
                      .catch((e) => message.error(String(e)));
                  }}
                  options={[
                    { value: 30, label: "保持 30 分钟" },
                    { value: 1440, label: "保持 1 天" },
                    { value: 0, label: "不自动锁定" },
                  ]}
                />
              </Tooltip>
            )}
          </Space>
          <div style={{ flex: 1, minWidth: 8 }} />
          <Space size={8} wrap>
            {phase === "locked" && (
              <Button size="small" type="primary" loading={busy} onClick={openUnlock}>
                解锁…
              </Button>
            )}
            {phase === "unlocked" && (
              <Button size="small" onClick={() => void lock()}>
                锁定
              </Button>
            )}
            {phase === "unlocked" && setPwCheck.ok && (
              <Button size="small" onClick={setMaster}>
                设置主密码…
              </Button>
            )}
            {phase === "unlocked" && status?.has_master_password && (
              <>
                <Button size="small" onClick={setMaster}>
                  更改主密码…
                </Button>
                {/* 移除主密码是危险操作：从常驻红按钮收进「···」，点开仍走
                    输入当前主密码的确认 Modal（后端校验，误触第一层被挡） */}
                <Dropdown
                  trigger={["click"]}
                  menu={{
                    items: [{ key: "remove-master", danger: true, label: "移除主密码" }],
                    onClick: ({ key }) => {
                      if (key === "remove-master") removeMaster();
                    },
                  }}
                >
                  <Button size="small" aria-label="更多主密码操作">
                    ···
                  </Button>
                </Dropdown>
              </>
            )}
            {phase === "unlocked" && storageAvailable(status) && (
              <Button size="small" type="primary" onClick={addSiteModal}>
                + 网站
              </Button>
            )}
          </Space>
        </div>
      </Card>

      {phase === "unprovisioned" && (
        <Card size="small" className="sk-card" title="创建密码库">
          <Space direction="vertical" size={10} style={{ width: "100%" }}>
            <Radio.Group value={newMode} onChange={(e) => setNewMode(e.target.value)}>
              <Radio value="password">主密码模式（推荐）</Radio>
              <Radio value="dpapi">免密模式（DPAPI，绑定当前 Windows 用户）</Radio>
            </Radio.Group>
            {newMode === "password" && (
              <>
                <Input.Password
                  style={{ maxWidth: 380 }}
                  placeholder="主密码（至少 8 位更稳）"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                />
                {password && (
                  <Tag
                    color={
                      passwordStrength(password).level === "good"
                        ? "green"
                        : passwordStrength(password).level === "fair"
                          ? "orange"
                          : "red"
                    }
                  >
                    {passwordStrength(password).hint}
                  </Tag>
                )}
              </>
            )}
            <Button type="primary" size="small" loading={busy} onClick={setup}>
              创建密码库
            </Button>
          </Space>
        </Card>
      )}

      {!storageAvailable(status) ? (
        // capability 门兜底（正常恒走解锁分支）：仅在后端异常时出现
        <Alert
          type="warning"
          showIcon
          message="密码库存取暂不可用"
          description="后端数据层异常，请查看日志并重启应用。"
        />
      ) : phase === "locked" ? (
        <Alert
          type="info"
          showIcon
          message="密码库已锁定"
          description="点右上角「解锁…」：免密模式直接打开；主密码模式输入主密码验证后查看与编辑条目。"
        />
      ) : phase === "unlocked" ? (
        <div style={{ display: "flex", gap: 12, alignItems: "stretch" }}>
          {/* 左栏：网站列表（固定 ~300px，搜索嵌在卡标题） */}
          <Card
            size="small"
            className="sk-card"
            style={{ width: 300, flexShrink: 0 }}
            styles={{ body: { padding: "4px 4px 8px" } }}
            title={
              <Input.Search
                size="small"
                allowClear
                placeholder="搜索站点…"
                style={{ width: 190 }}
                onSearch={(q) => void loadSites(q)}
              />
            }
          >
            <Table<SiteRow>
              size="small"
              rowKey="id"
              showHeader={false}
              loading={sitesLoading}
              dataSource={sites}
              pagination={false}
              onRow={(r) => ({
                onClick: () => setSelectedSite(r.id),
                style: { cursor: "pointer" },
              })}
              rowClassName={(r) => (r.id === selectedSite ? "ant-table-row-selected" : "")}
              columns={[
                {
                  key: "site",
                  render: (_: unknown, r) => (
                    <div
                      style={{
                        display: "flex",
                        alignItems: "baseline",
                        justifyContent: "space-between",
                        gap: 8,
                      }}
                    >
                      <Space size={6} style={{ minWidth: 0 }}>
                        <span style={{ fontWeight: 600 }}>{r.name}</span>
                        {r.domain && <span className="sk-hint">{r.domain}</span>}
                      </Space>
                      <Tag style={{ marginInlineEnd: 0 }}>{r.n_accounts}</Tag>
                    </div>
                  ),
                },
              ]}
              locale={{ emptyText: "暂无站点——点右上角「+ 网站」开始" }}
            />
          </Card>

          {/* 右栏：该站账号（flex:1；未选中站点时给引导文案） */}
          <Card
            size="small"
            className="sk-card"
            style={{ flex: 1, minWidth: 0 }}
            title={
              selSite ? (
                // 面包屑形态「站点 › 名称（共 N 个账号）[编辑站点]」：取代
                // 与左栏重复的站点名平铺；账号数随增删实时来自 sites 行。
                <Space size={6} wrap>
                  <span className="sk-hint">站点</span>
                  <span className="sk-hint">›</span>
                  <span style={{ fontWeight: 600 }}>{selSite.name}</span>
                  <span className="sk-hint">（共 {selSite.n_accounts} 个账号）</span>
                  <Button
                    type="link"
                    size="small"
                    style={{ paddingInline: 0 }}
                    onClick={() => siteModal(selSite)}
                  >
                    编辑站点
                  </Button>
                </Space>
              ) : (
                "账号"
              )
            }
            extra={
              selSite ? (
                <Space size={4}>
                  <Button size="small" type="primary" onClick={addAccountModal}>
                    + 账号
                  </Button>
                  {/* 删除站点：图标化但有 Tooltip；弹窗里仍列明波及账号数 */}
                  <Tooltip title={`删除站点「${selSite.name}」`}>
                    <Button size="small" danger onClick={() => confirmDeleteSite(selSite)}>
                      🗑️
                    </Button>
                  </Tooltip>
                </Space>
              ) : undefined
            }
          >
            {!selSite ? (
              <Typography.Text type="secondary">
                左侧点一个网站，这里会显示它的账号；还没有站点就点右上角「+ 网站」新建。
              </Typography.Text>
            ) : (
              <Space direction="vertical" size={8} style={{ width: "100%" }}>
                <Input.Search
                  size="small"
                  allowClear
                  placeholder="搜索账号：用户名 / 标签"
                  style={{ width: 260 }}
                  value={accSearch}
                  onChange={(e) => setAccSearch(e.target.value)}
                  onSearch={(q) => void loadAccounts(q)}
                />
                <Table<AccountRow>
                  size="small"
                  rowKey="id"
                  loading={accountsLoading}
                  dataSource={accounts}
                  pagination={false}
                  columns={[
                    {
                      title: "账号",
                      key: "title",
                      ellipsis: true,
                      render: (_: unknown, a) => (
                        <Space size={4}>
                          {a.is_favorite && <span>⭐</span>}
                          <span>{accountTitle(a)}</span>
                        </Space>
                      ),
                    },
                    {
                      title: "用户名",
                      key: "username",
                      ellipsis: true,
                      render: (_: unknown, a) =>
                        a.username ? (
                          <Tooltip title="双击复制用户名（不进剪贴板历史）">
                            <span
                              style={{ cursor: "copy", whiteSpace: "nowrap" }}
                              onDoubleClick={() =>
                                void invoke("sk_clip_copy_plain", { text: a.username })
                                  .then(() => message.success("用户名已复制"))
                                  .catch((e) => message.error(String(e)))
                              }
                            >
                              {a.username}
                            </span>
                          </Tooltip>
                        ) : (
                          <span className="sk-hint">—</span>
                        ),
                    },
                    {
                      // 密码列恒定纯掩码（固定 8 圆点，长度不外泄）+ 单行 ellipsis，
                      // 旁边才有 👁️/📋 两个小动作——此前「账号名紧贴掩码」在窄表里
                      // 读起来像「1111••••」混排（用户实锤），图标动作也让列内容
                      // 自成一体。双击复制语义保留。
                      title: "密码",
                      key: "password",
                      width: 190,
                      ellipsis: true,
                      render: (_: unknown, a) => {
                        const revealed = reveal?.id === a.id;
                        return (
                          <Space size={0} wrap={false}>
                            <Tooltip
                              title={
                                revealed
                                  ? "双击复制密码；点 👁️ 重新隐藏"
                                  : "双击复制密码（敏感通道，25 秒后自动清空剪贴板）"
                              }
                            >
                              <span
                                style={{
                                  cursor: "copy",
                                  display: "inline-block",
                                  verticalAlign: "bottom",
                                  whiteSpace: "nowrap", // 恒单行：掩码/明文都不得换行撑破布局
                                  overflow: "hidden",
                                  textOverflow: "ellipsis",
                                  maxWidth: 108,
                                  letterSpacing: revealed ? 0 : 2,
                                }}
                                onDoubleClick={() => void copySecret(a.id, "password")}
                              >
                                {revealed ? reveal?.text : maskedSecret()}
                              </span>
                            </Tooltip>
                            <Tooltip title={revealed ? "隐藏密码" : "临时显示明文（30 秒后自动隐藏）"}>
                              <Button
                                type="text"
                                size="small"
                                aria-label={revealed ? "隐藏密码" : "显示密码"}
                                onClick={() => void toggleReveal(a)}
                              >
                                {revealed ? "🙈" : "👁️"}
                              </Button>
                            </Tooltip>
                            <Tooltip title="复制密码（不进历史，25 秒后自动清空剪贴板）">
                              <Button
                                type="text"
                                size="small"
                                aria-label="复制密码"
                                onClick={() => void copySecret(a.id, "password")}
                              >
                                📋
                              </Button>
                            </Tooltip>
                          </Space>
                        );
                      },
                    },
                    {
                      title: "标签",
                      dataIndex: "label",
                      width: 140,
                      ellipsis: true,
                      render: (v: string) => (v ? <span>{v}</span> : <span className="sk-hint">—</span>),
                    },
                    {
                      // 操作列收进行尾「···」菜单：置顶/编辑/删除/动态码全部菜单化，
                      // 窄表不再被一排文字按钮挤压（复制密码的 📋 就在密码列掩码旁）。
                      // 删除仍带二次确认弹窗。
                      title: "操作",
                      key: "ops",
                      width: 64,
                      render: (_: unknown, a) => {
                        const items: MenuProps["items"] = [
                          ...(a.has_totp ? [{ key: "totp", label: "⏱ 动态码" }] : []),
                          { key: "fav", label: a.is_favorite ? "⭐ 取消置顶" : "📌 置顶" },
                          { key: "edit", label: "✏️ 编辑" },
                          { type: "divider" as const },
                          { key: "del", danger: true, label: "🗑️ 删除" },
                        ];
                        return (
                          <Dropdown
                            trigger={["click"]}
                            menu={{
                              items,
                              onClick: ({ key }) => {
                                if (key === "totp") showTotp(a);
                                else if (key === "fav") toggleFav(a);
                                else if (key === "edit") accountModal(a);
                                else if (key === "del") confirmDeleteAccount(a);
                              },
                            }}
                          >
                            <Button size="small" aria-label="更多操作">
                              ···
                            </Button>
                          </Dropdown>
                        );
                      },
                    },
                  ]}
                  locale={{ emptyText: "该站点暂无账号——点右上角「+ 账号」添加" }}
                />
              </Space>
            )}
          </Card>
        </div>
      ) : null}
    </Space>
  );
}
