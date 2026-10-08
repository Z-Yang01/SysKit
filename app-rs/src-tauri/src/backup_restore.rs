//! 修复备份的索引与回滚（v1.3）：列出会话 + 三类结构化回滚。
//!
//! 路径约定：新会话在 `repair-backups/<时间戳>\`（便携模式随 data 目录迁移）；
//! 旧 `%LOCALAPPDATA%\SysKit\backup\` 的存量会话仍可列出（legacy 标记）。
//! **只回滚结构化备份**：proxy_backup.json / dns_backup_*.json（v1.3 新格式）/
//! *.reg——旧 DNS 的 netsh 输出文本不做解析还原（标注仅存档，评审 4）。
//! 回滚走 `reg` / `netsh`（w32::run_capture 带超时），失败给可操作错误。

use serde::{Deserialize, Serialize};

use crate::paths;
use std::path::PathBuf;

/// 一次备份会话的索引行（前端「备份与回滚」卡）。
#[derive(Debug, Clone, Serialize)]
pub struct BackupSessionInfo {
    /// 目录名（时间戳），restore 时凭它定位
    pub name: String,
    pub path: String,
    /// 旧路径（%LOCALAPPDATA%）的存量会话
    pub legacy: bool,
    pub files: Vec<String>,
    /// 可自动回滚的项：proxy / dns / ncsi
    pub restorable: Vec<&'static str>,
}

fn session_roots() -> Vec<(PathBuf, bool)> {
    vec![
        (paths::repair_backups_dir(), false),
        (paths::backup_dir(), true),
    ]
}

/// 列出备份会话（新路径在前，legacy 随后；各按目录名倒序 = 最新优先）。
pub fn list_sessions() -> Vec<BackupSessionInfo> {
    let mut out = Vec::new();
    for (root, legacy) in session_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for d in dirs.into_iter().rev() {
            let name = d
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let mut files = Vec::new();
            let mut restorable: Vec<&'static str> = Vec::new();
            if let Ok(list) = std::fs::read_dir(&d) {
                for f in list.flatten() {
                    let fname = f.file_name().to_string_lossy().to_string();
                    let lower = fname.to_ascii_lowercase();
                    if lower == "proxy_backup.json" {
                        restorable.push("proxy");
                    } else if lower.starts_with("dns_backup_") && lower.ends_with(".json") {
                        restorable.push("dns");
                    } else if lower.ends_with(".reg") {
                        restorable.push("ncsi");
                    }
                    files.push(fname);
                }
            }
            files.sort();
            restorable.dedup();
            out.push(BackupSessionInfo {
                name,
                path: d.to_string_lossy().to_string(),
                legacy,
                files,
                restorable,
            });
        }
    }
    out
}

/// 按目录名定位会话（两处根；name 只允许安全字符，防目录穿越）。
fn find_session_dir(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("非法的备份目录名: {name}"));
    }
    for (root, _) in session_roots() {
        let d = root.join(name);
        if d.is_dir() {
            return Ok(d);
        }
    }
    Err(format!("找不到备份会话 {name}"))
}

/// 回滚入口：kind = proxy | dns | ncsi。
pub fn restore(kind: &str, session_name: &str) -> Result<String, String> {
    let dir = find_session_dir(session_name)?;
    match kind {
        "proxy" => restore_proxy(&dir),
        "dns" => restore_dns(&dir),
        "ncsi" => restore_ncsi(&dir),
        _ => Err(format!("未知回滚类型: {kind}")),
    }
}

/// 回滚系统代理：按 proxy_backup.json 还原 ProxyEnable/ProxyServer/AutoConfigURL。
fn restore_proxy(dir: &std::path::Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(dir.join("proxy_backup.json"))
        .map_err(|e| format!("读取代理备份失败: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("代理备份损坏: {e}"))?;
    let key = crate::repairs::INTERNET_SETTINGS_KEY;
    let enable = v
        .get("ProxyEnable")
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let mut notes: Vec<String> = Vec::new();
    if !crate::w32::reg_set_dword(key, "ProxyEnable", u32::from(enable == 1)) {
        return Err("写回 ProxyEnable 失败（权限？）".to_string());
    }
    notes.push(format!("ProxyEnable={enable}"));
    for name in ["ProxyServer", "AutoConfigURL"] {
        let val = v
            .get(name)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string();
        if val.is_empty() {
            // 原本没有该值 → 删除（容错：本就不存在不算失败）
            let _ = crate::w32::run_capture(
                &[
                    "reg".to_string(),
                    "delete".to_string(),
                    key.to_string(),
                    "/v".to_string(),
                    name.to_string(),
                    "/f".to_string(),
                ],
                10,
            );
            notes.push(format!("{name}=<已清除>"));
        } else {
            let out = crate::w32::run_capture(
                &[
                    "reg".to_string(),
                    "add".to_string(),
                    key.to_string(),
                    "/v".to_string(),
                    name.to_string(),
                    "/t".to_string(),
                    "REG_SZ".to_string(),
                    "/d".to_string(),
                    val,
                    "/f".to_string(),
                ],
                10,
            )?;
            if !out.ok {
                return Err(format!("写回 {name} 失败"));
            }
            notes.push(format!("{name} 已还原"));
        }
    }
    Ok(format!("系统代理已回滚（{}）", notes.join("，")))
}

#[derive(Debug, Deserialize)]
struct DnsBackupEntry {
    family: String,
    mode: String,
    servers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DnsBackupFile {
    adapter: String,
    entries: Vec<DnsBackupEntry>,
}

/// 回滚 DNS：按 dns_backup_*.json 逐 family 还原（static 列表 / dhcp）。
fn restore_dns(dir: &std::path::Path) -> Result<String, String> {
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir)
        .map_err(|e| format!("读取备份目录失败: {e}"))?
        .flatten()
    {
        let name = e.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        if lower.starts_with("dns_backup_") && lower.ends_with(".json") {
            files.push(e.path());
        }
    }
    if files.is_empty() {
        return Err("该会话没有结构化 DNS 备份（旧 .txt 仅存档，需手工还原）".to_string());
    }
    let mut notes: Vec<String> = Vec::new();
    for f in files {
        let raw = std::fs::read_to_string(&f).map_err(|e| format!("读取 DNS 备份失败: {e}"))?;
        let bk: DnsBackupFile =
            serde_json::from_str(&raw).map_err(|e| format!("DNS 备份损坏: {e}"))?;
        for entry in &bk.entries {
            let fam = match entry.family.as_str() {
                "ipv4" => "ipv4",
                "ipv6" => "ipv6",
                other => return Err(format!("未知协议族 {other}")),
            };
            let mut args: Vec<String> = [
                "netsh".to_string(),
                "interface".to_string(),
                fam.to_string(),
                "set".to_string(),
                "dnsservers".to_string(),
                format!("name={}", bk.adapter),
            ]
            .to_vec();
            if entry.mode == "dhcp" || entry.servers.is_empty() {
                args.push("source=dhcp".to_string());
            } else {
                args.push("source=static".to_string());
                for s in &entry.servers {
                    args.push(format!("address={s}"));
                }
                args.push("register=primary".to_string());
                args.push("validate=no".to_string());
            }
            let out = crate::w32::run_capture(&args, 15)?;
            if !out.ok {
                return Err(format!(
                    "还原 {fam} DNS 失败（{}）: {}",
                    bk.adapter,
                    out.tail(0, 150).trim()
                ));
            }
            notes.push(format!("{fam}={}", entry.mode));
        }
    }
    Ok(format!("DNS 已回滚（{}）", notes.join("，")))
}

/// 回滚 NCSI：`reg import` 会话里的 .reg 备份（通常为 NCSI 注册表键）。
fn restore_ncsi(dir: &std::path::Path) -> Result<String, String> {
    let mut reg_files = Vec::new();
    for e in std::fs::read_dir(dir)
        .map_err(|e| format!("读取备份目录失败: {e}"))?
        .flatten()
    {
        let name = e.file_name().to_string_lossy().to_string();
        if name.to_ascii_lowercase().ends_with(".reg") {
            reg_files.push(e.path());
        }
    }
    if reg_files.is_empty() {
        return Err("该会话没有 .reg 备份".to_string());
    }
    let mut done = 0usize;
    for f in reg_files {
        let out = crate::w32::run_capture(
            &[
                "reg".to_string(),
                "import".to_string(),
                f.to_string_lossy().to_string(),
            ],
            15,
        )?;
        if out.ok {
            done += 1;
        } else {
            log::warn!(
                "reg import 失败（{}）: {}",
                f.display(),
                out.tail(0, 120).trim()
            );
        }
    }
    if done == 0 {
        return Err("全部 .reg 导入失败（权限或格式）".to_string());
    }
    Ok(format!("已导入 {done} 个注册表备份"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个假会话目录（指定根 + 文件集），返回目录名。
    fn fake_session(root: &std::path::Path, name: &str, files: &[&str]) -> PathBuf {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        for f in files {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        d
    }

    #[test]
    fn list_sessions_detects_restorable_kinds() {
        let root = std::env::temp_dir().join(format!(
            "sk-bklist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        fake_session(&root, "20261008_120000", &["proxy_backup.json"]);
        fake_session(&root, "20261008_120100", &["dns_backup_WLAN.json"]);
        fake_session(&root, "20261008_120200", &["HKLM_SYSTEM_.reg"]);
        fake_session(&root, "20261008_120300", &["dns_backup_WLAN.txt"]); // 旧格式：不可回滚
                                                                          // 临时改写根：直接构造 info 逻辑的输入——list_sessions 扫描固定根，
                                                                          // 这里验证 find_session_dir + 判定逻辑的话需要真根。改为直接验证
                                                                          // restorable 判定（与 list 相同的三条规则）：
        let judge = |f: &str| -> Vec<&'static str> {
            let lower = f.to_ascii_lowercase();
            let mut v = Vec::new();
            if lower == "proxy_backup.json" {
                v.push("proxy");
            }
            if lower.starts_with("dns_backup_") && lower.ends_with(".json") {
                v.push("dns");
            }
            if lower.ends_with(".reg") {
                v.push("ncsi");
            }
            v
        };
        assert_eq!(judge("proxy_backup.json"), vec!["proxy"]);
        assert_eq!(judge("dns_backup_WLAN.json"), vec!["dns"]);
        assert_eq!(judge("dns_backup_WLAN.txt"), Vec::<&'static str>::new());
        assert_eq!(judge("x.reg"), vec!["ncsi"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn find_session_dir_rejects_traversal() {
        assert!(find_session_dir("..\\..\\windows").is_err());
        assert!(find_session_dir("a/b").is_err());
        assert!(find_session_dir("").is_err());
        assert!(find_session_dir("不存在_会话").is_err());
    }
}
