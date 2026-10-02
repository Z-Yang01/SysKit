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
  Empty,
  Input,
  Modal,
  Radio,
  Space,
  Tag,
  Typography,
  message,
} from "antd";

import {
  canAddPassword,
  canSetMasterPassword,
  modeLabel,
  passwordStrength,
  setupConfirmText,
  statusSummary,
  storageAvailable,
  storageNotice,
  vaultPhase,
  type VaultStatus,
} from "./vault";

export function VaultPage() {
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [password, setPassword] = useState("");
  const [newMode, setNewMode] = useState<"dpapi" | "password">("password");
  const [busy, setBusy] = useState(false);

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

  const phase = vaultPhase(status);
  const notice = storageNotice(status);
  const addCheck = canAddPassword(status);
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

      <Card size="small" className="sk-card" title="密码条目">
        <Space direction="vertical" size={10} style={{ width: "100%" }}>
          <Space>
            <Button size="small" type="primary" disabled={!addCheck.ok} onClick={() => void invoke("sk_vault_store_unavailable").catch((e) => message.error(String(e)))}>
              添加密码
            </Button>
            <span className="sk-hint">{addCheck.ok ? "" : addCheck.reason}</span>
          </Space>
          <Divider style={{ margin: "4px 0" }} />
          {storageAvailable(status) ? (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无密码条目" />
          ) : (
            // 不渲染空列表：那看起来像"还没有条目"，而不是"功能未做"
            <Alert
              type="info"
              showIcon
              message="此处不会显示任何条目"
              description="数据层未引入，因此本页既不展示条目也不提供添加表单——避免让「空列表」被误读为「还没有密码」。加密层已完成，vault.key 与 Python 版双向互通。"
            />
          )}
        </Space>
      </Card>
    </Space>
  );
}
