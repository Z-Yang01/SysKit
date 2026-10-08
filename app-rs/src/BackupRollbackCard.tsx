/**
 * 备份与回滚卡（诊断/修复页）：列出备份会话，一键回滚结构化项。
 *
 * 回滚是危险操作（真实写注册表/netsh）：每项走确认弹窗，结果原样展示。
 * legacy（旧 %LOCALAPPDATA% 路径）会话同样可回滚（文件判定一致）。
 */
import { useCallback, useEffect, useState } from "react";
import {
  Alert,
  Button,
  Modal,
  Row,
  Space,
  Table,
  Tag,
  Typography,
  message,
} from "antd";

import {
  listBackups,
  restorableLabel,
  restoreBackup,
  sessionTimeText,
  type BackupSessionInfo,
} from "./backups";

export function BackupRollbackCard() {
  const [sessions, setSessions] = useState<BackupSessionInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setSessions(await listBackups());
    } catch (e) {
      message.error(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const restore = useCallback(
    async (kind: string, sessionName: string) => {
      setBusy(`${kind}:${sessionName}`);
      try {
        const msg = await restoreBackup(kind, sessionName);
        message.success(msg || "已回滚");
      } catch (e) {
        message.error(String(e));
      } finally {
        setBusy("");
      }
    },
    [],
  );

  const confirmRestore = useCallback(
    (kind: string, sessionName: string, label: string) => {
      Modal.confirm({
        title: `回滚${label}？`,
        width: 480,
        content:
          "将把系统配置恢复为该次修复动作执行前的备份原值（真实写注册表/netsh）。确定继续？",
        okText: "回滚",
        okButtonProps: { danger: true },
        cancelText: "取消",
        onOk: () => restore(kind, sessionName),
      });
    },
    [restore],
  );

  return (
    <div className="sk-card">
      <Row justify="space-between" align="middle" style={{ marginBottom: 8 }}>
        <Typography.Text strong>备份与回滚</Typography.Text>
        <Button size="small" loading={loading} onClick={() => void load()}>
          刷新
        </Button>
      </Row>
      <Typography.Text type="secondary" className="sk-hint" style={{ display: "block", marginBottom: 8 }}>
        每次修复动作改配置前都会先备份原值。这里按会话列出，可一键还原结构化项
        （系统代理 / DNS / NCSI 注册表）。
      </Typography.Text>
      {sessions.length === 0 && !loading ? (
        <Alert type="info" showIcon message="还没有备份会话——执行任一改配置类修复动作后出现在这里" />
      ) : (
        <Table<BackupSessionInfo>
          size="small"
          rowKey={(r) => `${r.legacy ? "L" : "N"}-${r.name}`}
          loading={loading}
          pagination={false}
          dataSource={sessions.slice(0, 10)}
          columns={[
            {
              title: "时间",
              dataIndex: "name",
              width: 150,
              render: (v: string, r) => (
                <Space size={6}>
                  {sessionTimeText(v)}
                  {r.legacy && <Tag>旧位置</Tag>}
                </Space>
              ),
            },
            {
              title: "可回滚项",
              dataIndex: "restorable",
              render: (v: string[], r) =>
                v.length === 0 ? (
                  <span className="sk-hint">无可自动回滚项（{r.files.length} 个文件仅存档）</span>
                ) : (
                  <Space wrap size={4}>
                    {v.map((k) => (
                      <Button
                        key={k}
                        size="small"
                        loading={busy === `${k}:${r.name}`}
                        onClick={() => confirmRestore(k, r.name, restorableLabel(k))}
                      >
                        回滚{restorableLabel(k)}
                      </Button>
                    ))}
                  </Space>
                ),
            },
          ]}
        />
      )}
    </div>
  );
}
