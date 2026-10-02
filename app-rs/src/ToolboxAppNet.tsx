/**
 * 工具箱：应用联网（对应 Python 侧 AppNetTab）。
 *
 * 用 Windows 防火墙出站规则按程序阻断联网。规则名由 `SysKit-AppNet-<sha1>` 生成，
 * **与 Python 版逐字节一致**——两版共享 `appnet.json` 台账与同一批系统规则，
 * 迁移期间不会产生重复规则。
 *
 * 三态展示：已禁止 / 已放行 / 已失效（动作被外部改成"允许"）。最后一种必须显眼——
 * 否则用户以为程序被拦住了，实际它正畅通无阻。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Card,
  Empty,
  Input,
  Modal,
  Space,
  Table,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";

import {
  adminNotice,
  appNetStats,
  attentionNotice,
  baseName,
  blockConfirmText,
  dirName,
  isReallyBlocking,
  needsAttention,
  stateColor,
  stateLabel,
  type AppNetList,
  type AppNetRow,
  type AppNetValidate,
} from "./appnet";

export function ToolboxAppNet() {
  const [list, setList] = useState<AppNetList | null>(null);
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [validation, setValidation] = useState<AppNetValidate | null>(null);

  const load = useCallback(async () => {
    try {
      setList(await invoke<AppNetList>("sk_appnet_list"));
    } catch (e) {
      message.error(`读取失败：${String(e)}`);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // 路径变化即时校验（含规则名与错误原因）
  useEffect(() => {
    if (!path.trim()) {
      setValidation(null);
      return;
    }
    let alive = true;
    void invoke<AppNetValidate>("sk_appnet_validate", { program: path })
      .then((v) => {
        if (alive) setValidation(v);
      })
      .catch(() => {
        if (alive) setValidation(null);
      });
    return () => {
      alive = false;
    };
  }, [path]);

  const rows = list?.rows ?? [];
  const stats = useMemo(() => appNetStats(rows), [rows]);
  const notice = useMemo(() => attentionNotice(rows), [rows]);
  const adminMsg = adminNotice(list, "修改防火墙规则需要管理员权限。请以管理员身份运行 SysKit 后重试。");

  const pick = useCallback(async () => {
    try {
      // 走 Win32 原生对话框（后端 GetOpenFileNameW），不为一次选择引入插件依赖
      const picked = await invoke<string>("sk_appnet_pick_exe");
      if (picked) setPath(picked);
    } catch (e) {
      message.error(`选择文件失败：${String(e)}`);
    }
  }, []);

  const apply = useCallback(
    async (program: string, blocked: boolean) => {
      setBusy(true);
      try {
        const r = await invoke<{ message: string }>("sk_appnet_add", { program, blocked });
        message.success(r.message);
        setPath("");
        await load();
      } catch (e) {
        message.error(String(e));
      } finally {
        setBusy(false);
      }
    },
    [load],
  );

  const add = useCallback(() => {
    if (!validation || validation.error) {
      message.warning(validation?.error || "请先选择或输入程序路径");
      return;
    }
    Modal.confirm({
      title: "禁止程序联网",
      width: 560,
      content: (
        <Typography.Paragraph style={{ whiteSpace: "pre-wrap" }}>
          {blockConfirmText(validation.program)}
        </Typography.Paragraph>
      ),
      okText: "禁止联网",
      okButtonProps: { danger: true },
      cancelText: "取消",
      onOk: () => apply(validation.program, true),
    });
  }, [validation, apply]);

  const remove = useCallback(
    (row: AppNetRow) => {
      Modal.confirm({
        title: "移除联网控制",
        content: `将删除 ${baseName(row.program)} 的防火墙规则并从台账移除。`,
        okText: "移除",
        okButtonProps: { danger: true },
        cancelText: "取消",
        onOk: async () => {
          try {
            message.success(await invoke<string>("sk_appnet_remove", { program: row.program }));
            await load();
          } catch (e) {
            message.error(String(e));
          }
        },
      });
    },
    [load],
  );

  return (
    <Space direction="vertical" size={12} style={{ width: "100%" }}>
      {adminMsg && <Alert type="warning" showIcon message={adminMsg} />}
      {notice && <Alert type="error" showIcon message="规则状态异常" description={notice} />}

      <Card
        size="small"
        className="sk-card"
        title="添加受控程序"
        extra={
          <Space>
            <Tag color="green">禁止 {stats.blocking}</Tag>
            <Tag>放行 {stats.released}</Tag>
            {stats.attention > 0 && <Tag color="red">异常 {stats.attention}</Tag>}
            <Button size="small" onClick={() => void load()}>
              刷新
            </Button>
          </Space>
        }
      >
        <Space wrap>
          <Input
            size="small"
            style={{ width: 380 }}
            placeholder="程序完整路径，如 C:\Program Files\App\app.exe"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            onPressEnter={add}
          />
          <Button size="small" onClick={() => void pick()}>
            浏览…
          </Button>
          <Button size="small" type="primary" danger loading={busy} onClick={add}>
            禁止联网
          </Button>
          {validation && (
            <span className="sk-hint">
              {validation.error ? (
                <Typography.Text type="danger">{validation.error}</Typography.Text>
              ) : (
                <>
                  规则名 <code>{validation.rule}</code>
                </>
              )}
            </span>
          )}
        </Space>
      </Card>

      <Card size="small" className="sk-card" title="受控程序">
        {rows.length === 0 ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description="还没有受控程序。上面选一个 exe 就能禁止它联网。"
          />
        ) : (
          <Table<AppNetRow>
            size="small"
            rowKey="rule"
            pagination={{ pageSize: 15, showSizeChanger: false }}
            dataSource={rows}
            columns={[
              {
                title: "程序",
                render: (_: unknown, r) => (
                  <Space direction="vertical" size={0}>
                    <span>{baseName(r.program)}</span>
                    <span className="sk-hint">{dirName(r.program)}</span>
                  </Space>
                ),
              },
              {
                title: "状态",
                dataIndex: "state",
                width: 210,
                render: (v: RuleStateLike, r) => (
                  <Tooltip title={needsAttention(r) ? "台账与实际不一致，请处理" : r.rule}>
                    <Tag color={stateColor(v)}>{stateLabel(v)}</Tag>
                  </Tooltip>
                ),
              },
              { title: "加入时间", dataIndex: "added", width: 160 },
              {
                title: "操作",
                width: 180,
                render: (_: unknown, r) => (
                  <Space size={4}>
                    {isReallyBlocking(r) ? (
                      <Button size="small" type="link" onClick={() => void apply(r.program, false)}>
                        放行
                      </Button>
                    ) : (
                      <Button
                        size="small"
                        type="link"
                        loading={busy}
                        onClick={() => void apply(r.program, true)}
                      >
                        重新禁止
                      </Button>
                    )}
                    <Button size="small" type="link" danger onClick={() => remove(r)}>
                      移除
                    </Button>
                  </Space>
                ),
              },
            ]}
          />
        )}
      </Card>
    </Space>
  );
}

/** 局部类型别名（避免与 dnstools 的 RuleState 命名冲突时读者困惑）。 */
type RuleStateLike = AppNetRow["state"];
