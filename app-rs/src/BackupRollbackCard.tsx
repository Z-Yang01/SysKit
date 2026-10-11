/**
 * 备份与回滚卡（诊断/修复页）：列出备份会话，一键回滚结构化项。
 *
 * 回滚是危险操作（真实写注册表/netsh）：每项走确认弹窗，结果原样展示。
 * legacy（旧 %LOCALAPPDATA% 路径）会话同样可回滚（文件判定一致）。
 *
 * v1.4 UI 迁移：脱离 AntD（Table/Modal.confirm/message/Tag → ui 件）。
 */
import { useCallback, useEffect, useState } from "react";

import {
  listBackups,
  restorableLabel,
  restoreBackup,
  sessionTimeText,
  type BackupSessionInfo,
} from "./backups";
import { Badge, Button, Card, CardBody, CardHeader, CardTitle, Empty, Table, type Column } from "./ui";
import { Alert } from "./ui/alert";
import { confirm } from "./ui/confirm";
import { useToast } from "./ui/toast";

export function BackupRollbackCard() {
  const toast = useToast();
  const [sessions, setSessions] = useState<BackupSessionInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setSessions(await listBackups());
    } catch (e) {
      toast.error(String(e));
    } finally {
      setLoading(false);
    }
  }, [toast]);

  useEffect(() => {
    void load();
  }, [load]);

  const restore = useCallback(
    async (kind: string, sessionName: string) => {
      setBusy(`${kind}:${sessionName}`);
      try {
        const msg = await restoreBackup(kind, sessionName);
        toast.success(msg || "已回滚");
      } catch (e) {
        toast.error(String(e));
      } finally {
        setBusy("");
      }
    },
    [toast],
  );

  const confirmRestore = useCallback(
    (kind: string, sessionName: string, label: string) => {
      confirm({
        title: `回滚${label}？`,
        width: 480,
        content:
          "将把系统配置恢复为该次修复动作执行前的备份原值（真实写注册表/netsh）。确定继续？",
        okText: "回滚",
        danger: true,
        onOk: () => restore(kind, sessionName),
      });
    },
    [restore],
  );

  const columns: Column<BackupSessionInfo>[] = [
    {
      title: "时间",
      dataIndex: "name",
      width: 150,
      render: (v: string, r) => (
        <span className="inline-flex items-center gap-1.5">
          {sessionTimeText(v)}
          {r.legacy && <Badge>旧位置</Badge>}
        </span>
      ),
    },
    {
      title: "可回滚项",
      dataIndex: "restorable",
      render: (v: string[], r) =>
        v.length === 0 ? (
          <span className="sk-hint">无可自动回滚项（{r.files.length} 个文件仅存档）</span>
        ) : (
          <span className="inline-flex flex-wrap gap-1">
            {v.map((k) => (
              <Button
                key={k}
                size="sm"
                loading={busy === `${k}:${r.name}`}
                onClick={() => confirmRestore(k, r.name, restorableLabel(k))}
              >
                回滚{restorableLabel(k)}
              </Button>
            ))}
          </span>
        ),
    },
  ];

  return (
    <Card>
      <CardHeader>
        <CardTitle>备份与回滚</CardTitle>
        <Button size="sm" loading={loading} onClick={() => void load()}>
          刷新
        </Button>
      </CardHeader>
      <CardBody>
        <div className="sk-hint mb-2">
          每次修复动作改配置前都会先备份原值。这里按会话列出，可一键还原结构化项
          （系统代理 / DNS / NCSI 注册表）。
        </div>
        {sessions.length === 0 && !loading ? (
          <Alert tone="info" title="还没有备份会话——执行任一改配置类修复动作后出现在这里" />
        ) : (
          <Table<BackupSessionInfo>
            rowKey={(r) => `${r.legacy ? "L" : "N"}-${r.name}`}
            loading={loading}
            dataSource={sessions.slice(0, 10)}
            columns={columns}
            empty={<Empty description="还没有备份会话" />}
          />
        )}
      </CardBody>
    </Card>
  );
}
