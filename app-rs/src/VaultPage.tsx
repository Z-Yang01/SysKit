/**
 * 密码库页（M6：加密层 + 界面骨架）。
 *
 * ## 当前阶段的诚实边界
 *
 * 数据层（`vault.db` / SQLite）按决定**未实现**，所以本页：
 * - **不渲染任何密码条目，也不提供添加表单**——显示一个永远为空的列表看起来像
 *   "还没有条目"，而不是"功能没做"；
 * - 数据层不可用时，存取相关按钮禁用并显示明确原因；
 * - 后端 `sk_vault_store_unavailable` 是唯一的"存取入口"，它**明确失败**。
 *
 * 已可用的部分：`vault.key` 的创建/解锁/锁定，以及主密码的设置/校验/移除——
 * 这些是加密层的真实能力，且已通过双向跨语言验证。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Divider,
  Input,
  Modal,
  Radio,
  Space,
  Table,
  Tag,
  Typography,
  message,
} from "antd";

import {
  accountTitle,
  canSetMasterPassword,
  maskedSecret,
  modeLabel,
  passwordStrength,
  setupConfirmText,
  statusSummary,
  storageAvailable,
  storageNotice,
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
  const phase = vaultPhase(status);

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

  const loadAccounts = useCallback(async () => {
    if (selectedSite === null) return;
    setAccountsLoading(true);
    try {
      setAccounts(await invoke<AccountRow[]>("sk_vault_site_accounts", { siteId: selectedSite, q: "" }));
    } catch {
      setAccounts([]);
    } finally {
      setAccountsLoading(false);
    }
  }, [selectedSite]);

  useEffect(() => {
    // 解锁状态下首次拉站点；锁定/未建库时清空
    if (phase === "unlocked") void loadSites(search);
    else {
      setSites([]);
      setAccounts([]);
      setSelectedSite(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase]);

  useEffect(() => {
    if (selectedSite !== null) void loadAccounts();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedSite]);

  const copySecret = useCallback(async (accountId: number, field: string) => {
    const r = await invoke<{ note: string; ttl: number }>("sk_vault_copy_secret", {
      accountId,
      field,
    });
    message.success(r.ttl > 0 ? `已复制；${r.ttl} 秒后自动清空剪贴板` : "已复制（不会进入历史）");
  }, []);

  const showTotp = useCallback(
    (a: AccountRow) => {
      let info = message.open({
        type: "loading",
        content: "正在生成动态码…",
        duration: 0,
      });
      const tick = () => {
        void invoke<string>("sk_vault_totp_code", { accountId: a.id })
          .then((code) => {
            if (!code) {
              message.error("TOTP 密钥无效（含字母表外字符？）");
              return;
            }
            info();
            const remain = 30 - (Math.floor(Date.now() / 1000) % 30);
            info = message.open({
              type: "info",
              content: `${accountTitle(a)} 动态码：${code}（${remain}s 后刷新）`,
              duration: Math.max(remain, 1),
            });
          })
          .catch((e) => {
            message.error(String(e));
          });
      };
      tick();
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
            <Input id="vf-site-url" defaultValue={form.url} placeholder="URL"
                   onChange={(e) => (form.url = e.target.value)} />
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
              label: form.label,
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


  const notice = storageNotice(status);
  const setPwCheck = canSetMasterPassword(status);

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

  const unlock = useCallback(async () => {
    setBusy(true);
    try {
      const r = await invoke<{ message: string }>("sk_vault_unlock", {
        password: status?.has_master_password ? password : null,
      });
      message.success(r.message);
      setPassword("");
      await load();
    } catch (e) {
      // 后端区分「密码错误」与「文件损坏」，原样展示
      message.error(String(e));
    } finally {
      setBusy(false);
    }
  }, [password, status, load]);

  const lock = useCallback(async () => {
    await invoke("sk_vault_lock");
    message.success("已锁定");
    setPassword("");
    await load();
  }, [load]);

  const setMaster = useCallback(() => {
    Modal.confirm({
      title: "设置主密码",
      width: 520,
      content: (
        <Space direction="vertical" size={8} style={{ width: "100%" }}>
          <Typography.Paragraph style={{ whiteSpace: "pre-wrap", marginBottom: 0 }}>
            {setupConfirmText("password")}
          </Typography.Paragraph>
          <Input.Password
            placeholder="主密码（至少 8 位更稳）"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
          {password && (
            <Tag color={passwordStrength(password).level === "good" ? "green" : passwordStrength(password).level === "fair" ? "orange" : "red"}>
              {passwordStrength(password).hint}
            </Tag>
          )}
        </Space>
      ),
      okText: "设置",
      cancelText: "取消",
      onOk: async () => {
        try {
          message.success(await invoke<string>("sk_vault_set_password", { password }));
          setPassword("");
          await load();
        } catch (e) {
          message.error(String(e));
          throw e; // 保持对话框打开，用户可修正
        }
      },
    });
  }, [password, load]);

  const removeMaster = useCallback(() => {
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
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </Space>
      ),
      okText: "移除",
      okButtonProps: { danger: true },
      cancelText: "取消",
      onOk: async () => {
        try {
          const ok = await invoke<boolean>("sk_vault_remove_password", { current: password });
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
      <Alert
        type="warning"
        showIcon
        message="密码条目存取尚未实现（数据层未引入）"
        description={notice}
      />

      <Card
        size="small"
        className="sk-card"
        title="密码库状态"
        extra={
          <Space>
            <Tag color={phase === "unlocked" ? "green" : phase === "locked" ? "orange" : "default"}>
              {statusSummary(status)}
            </Tag>
            <Button size="small" onClick={() => void load()}>
              刷新
            </Button>
          </Space>
        }
      >
        <Descriptions
          size="small"
          column={1}
          items={[
            { key: "path", label: "密钥文件", children: <code>{status?.path ?? "—"}</code> },
            { key: "mode", label: "保护模式", children: modeLabel(status?.mode ?? "") },
            {
              key: "state",
              label: "解锁状态",
              children: status?.unlocked ? <Tag color="green">已解锁</Tag> : <Tag>已锁定</Tag>,
            },
          ]}
        />
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

      {phase === "locked" && (
        <Card size="small" className="sk-card" title="解锁">
          <Space wrap>
            {status?.has_master_password ? (
              <Input.Password
                style={{ width: 320 }}
                placeholder="主密码"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                onPressEnter={() => void unlock()}
              />
            ) : (
              <Typography.Text type="secondary">
                免密模式（DPAPI）：无需输入密码，点「解锁」即可。
              </Typography.Text>
            )}
            <Button type="primary" size="small" loading={busy} onClick={() => void unlock()}>
              解锁
            </Button>
          </Space>
        </Card>
      )}

      {phase === "unlocked" && (
        <Card size="small" className="sk-card" title="主密码管理">
          <Space wrap>
            <Button size="small" onClick={() => void lock()}>
              锁定
            </Button>
            {setPwCheck.ok ? (
              <Button size="small" onClick={setMaster}>
                设置主密码
              </Button>
            ) : (
              <Button size="small" danger onClick={removeMaster}>
                移除主密码
              </Button>
            )}
            <span className="sk-hint">{setPwCheck.ok ? "" : setPwCheck.reason}</span>
          </Space>
        </Card>
      )}

      <Card
        size="small"
        className="sk-card"
        title="密码条目"
        extra={
          <Space>
            <Input.Search
              size="small"
              allowClear
              placeholder="搜索站点…"
              style={{ width: 200 }}
              onSearch={(q) => void loadSites(q)}
            />
            {storageAvailable(status) && phase === "unlocked" && (
              <Button size="small" type="primary" onClick={addSiteModal}>
                添加站点
              </Button>
            )}
          </Space>
        }
      >
        {!storageAvailable(status) ? (
          // 数据层不可用：不渲染空列表（那看起来像"还没有条目"，而非"功能没做"）
          <Alert
            type="info"
            showIcon
            message="此处不会显示任何条目"
            description={notice || "数据层未引入，因此本页既不展示条目也不提供添加表单。"}
          />
        ) : phase !== "unlocked" ? (
          <Alert
            type="info"
            showIcon
            message="解锁后显示密码条目"
            description="条目列表需要解锁密码库；解锁后本页直接读库展示站点与账号。"
          />
        ) : (
          <Space direction="vertical" size={10} style={{ width: "100%" }}>
            <Table<SiteRow>
              size="small"
              rowKey="id"
              loading={sitesLoading}
              columns={[
                {
                  title: "站点",
                  dataIndex: "name",
                  render: (v: string, r) => (
                    <Space size={4}>
                      <span style={{ fontWeight: 600 }}>{v}</span>
                      {r.domain && <span className="sk-hint">{r.domain}</span>}
                    </Space>
                  ),
                },
                { title: "账号数", dataIndex: "n_accounts", width: 80 },
                {
                  title: "操作",
                  key: "ops",
                  width: 190,
                  render: (_: unknown, r) => (
                    <Space size={4}>
                      <Button
                        size="small"
                        type={selectedSite === r.id ? "primary" : "default"}
                        onClick={() => setSelectedSite(r.id)}
                      >
                        账号
                      </Button>
                      <Button size="small" onClick={() => siteModal(r)}>
                        编辑
                      </Button>
                      <Button
                        size="small"
                        danger
                        onClick={() =>
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
                          })
                        }
                      >
                        删除
                      </Button>
                    </Space>
                  ),
                },
              ]}
              dataSource={sites}
              pagination={false}
              locale={{ emptyText: "暂无站点——点右上角「添加站点」开始" }}
            />

            {selectedSite !== null && (
              <>
                <Divider style={{ margin: "4px 0" }} />
                <Space>
                  <Typography.Text strong>
                    {sites.find((s) => s.id === selectedSite)?.name ?? "账号"}
                  </Typography.Text>
                  <Button size="small" onClick={addAccountModal}>
                    添加账号
                  </Button>
                </Space>
                <Table<AccountRow>
                  size="small"
                  rowKey="id"
                  loading={accountsLoading}
                  columns={[
                    {
                      title: "账号",
                      key: "title",
                      render: (_: unknown, a) => (
                        <Space size={4}>
                          {a.is_favorite && <span>⭐</span>}
                          <span>{accountTitle(a)}</span>
                          {a.username && a.username !== a.label && (
                            <span className="sk-hint">{a.username}</span>
                          )}
                        </Space>
                      ),
                    },
                    {
                      title: "密文",
                      key: "secret",
                      width: 120,
                      render: () => <span className="sk-hint">{maskedSecret()}</span>,
                    },
                    {
                      title: "操作",
                      key: "ops",
                      width: 320,
                      render: (_: unknown, a) => (
                        <Space size={4}>
                          <Button size="small" onClick={() => void copySecret(a.id, "password")}>
                            复制密码
                          </Button>
                          {a.has_totp && (
                            <Button size="small" onClick={() => void showTotp(a)}>
                              动态码
                            </Button>
                          )}
                          <Button
                            size="small"
                            onClick={() =>
                              void invoke("sk_clip_copy_plain", { text: a.username })
                                .then(() => message.success("用户名已复制"))
                                .catch((e) => message.error(String(e)))
                            }
                          >
                            复制用户名
                          </Button>
                          <Button
                            size="small"
                            onClick={() =>
                              void invoke<boolean>("sk_vault_toggle_favorite", { accountId: a.id })
                                .then(() => void loadAccounts())
                                .catch((e) => message.error(String(e)))
                            }
                          >
                            {a.is_favorite ? "取消置顶" : "置顶"}
                          </Button>
                          <Button size="small" onClick={() => accountModal(a)}>
                            编辑
                          </Button>
                          <Button
                            size="small"
                            danger
                            onClick={() =>
                              Modal.confirm({
                                title: `删除账号「${accountTitle(a)}」`,
                                okText: "删除",
                                okButtonProps: { danger: true },
                                cancelText: "取消",
                                onOk: async () => {
                                  await invoke("sk_vault_delete_account", { accountId: a.id });
                                  await loadAccounts();
                                },
                              })
                            }
                          >
                            删除
                          </Button>
                        </Space>
                      ),
                    },
                  ]}
                  dataSource={accounts}
                  pagination={false}
                  locale={{ emptyText: "该站点暂无账号" }}
                />
              </>
            )}
          </Space>
        )}
      </Card>
    </Space>
  );
}
