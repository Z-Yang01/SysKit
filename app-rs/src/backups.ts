/**
 * 修复备份的索引与回滚（v1.3）：列出会话 + 三类结构化回滚。
 *
 * 路径约定：新会话在 `repair-backups/<时间戳>`（便携模式随 data 目录迁移）；
//! 旧 `%LOCALAPPDATA%\SysKit\backup` 的存量会话仍可列出（legacy 标记）。
 * **只回滚结构化备份**：proxy_backup.json / dns_backup_*.json（v1.3 新格式）/
 * *.reg——旧 DNS 的 netsh 输出文本不做解析还原（标注仅存档）。
 * 回滚走 `reg` / `netsh`（run_capture 带超时），失败给可操作错误。
 */
import { invoke } from "@tauri-apps/api/core";

export interface BackupSessionInfo {
  name: string;
  path: string;
  legacy: boolean;
  files: string[];
  restorable: ("proxy" | "dns" | "ncsi")[];
}

export function listBackups(): Promise<BackupSessionInfo[]> {
  return invoke<BackupSessionInfo[]>("sk_backup_list");
}

export function restoreBackup(kind: string, session: string): Promise<string> {
  return invoke<string>("sk_backup_restore", { kind, session });
}

/** 可回滚项的中文标签。 */
export function restorableLabel(kind: string): string {
  switch (kind) {
    case "proxy":
      return "系统代理";
    case "dns":
      return "DNS 配置";
    case "ncsi":
      return "NCSI 注册表";
    default:
      return kind;
  }
}

/** 会话时间显示：目录名 YYYYmmdd_HHMMSS → 2026-10-08 12:00。 */
export function sessionTimeText(name: string): string {
  const m = /^(\d{4})(\d{2})(\d{2})_(\d{2})(\d{2})(\d{2})$/.exec(name);
  if (!m) return name;
  return `${m[1]}-${m[2]}-${m[3]} ${m[4]}:${m[5]}`;
}
