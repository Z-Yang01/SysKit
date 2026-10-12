//! 修复动作的改前备份：每次执行开一个带时间戳的目录，把原值落盘。
//!
//! 与 Python 侧 `repairs._backup_dir/_save_note/_reg_export` 同语义：
//! `%LOCALAPPDATA%\SysKit\backup\<YYYYmmdd_HHMMSS>\`。
//!
//! **为什么备份失败必须让动作失败**（Python 注释里的原话）：改前备份是动作描述
//! 对用户的承诺。备份失败却照改，用户唯一的还原凭据就被静默抹掉了——
//! `dns_auto`/`dns_preset` 会清掉静态 DNS 原值，没有备份就再也回不去。
//! 因此 `BackupSession::new()` 失败时调用方必须中止动作，而不是继续。

use std::path::{Path, PathBuf};

use crate::paths;
use crate::timefmt;
use crate::w32;

/// 一次修复动作的备份会话。
#[derive(Debug, Clone)]
pub struct BackupSession {
    dir: PathBuf,
}

impl BackupSession {
    /// 开一个备份目录（`repair-backups/<YYYYmmdd_HHMMSS>`，v1.3 起新路径）。
    pub fn new() -> Result<Self, String> {
        let stamp = timefmt::now_fmt("%Y%m%d_%H%M%S");
        let dir = paths::repair_backups_dir().join(stamp);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("无法创建备份目录 {}: {e}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 把一段原文写进备份目录（对应 Python `_save_note`）；返回落盘路径。
    pub fn save_note(&self, name: &str, content: &str) -> Result<String, String> {
        let path = self.dir.join(sanitize_name(name));
        std::fs::write(&path, content)
            .map_err(|e| format!("备份写入失败 {}: {e}", path.display()))?;
        Ok(path.to_string_lossy().to_string())
    }

    /// 备份注册表键为 `.reg`（对应 Python `_reg_export`，走 `reg export /y`）。
    ///
    /// `reg_path_hklm` 形如 `HKLM\SYSTEM\...`。失败返回 Err（调用方据此中止动作）。
    pub fn export_reg(&self, reg_path_hklm: &str) -> Result<String, String> {
        let fname = format!(
            "{}.reg",
            sanitize_name(reg_path_hklm)
                .chars()
                .take(60)
                .collect::<String>()
        );
        let path = self.dir.join(fname);
        let out = w32::run_capture(
            &[
                "reg".into(),
                "export".into(),
                reg_path_hklm.into(),
                path.to_string_lossy().to_string(),
                "/y".into(),
            ],
            15,
        )
        .map_err(|e| format!("reg export 无法执行: {e}"))?;
        if !out.ok {
            return Err(format!(
                "reg export 失败（{}）: {}",
                reg_path_hklm,
                out.tail(0, 150).trim()
            ));
        }
        // **不能只看退出码**：实测 reg export 在部分键上退出码 0 却写出 0 字节文件。
        // 备份内容为空等于没备份——若据此放行动作，用户就没有还原凭据了。
        let bytes = std::fs::read(&path).unwrap_or_default();
        if bytes.is_empty() {
            return Err(format!(
                "reg export 产出的备份文件为空（{}），视为备份未完成",
                reg_path_hklm
            ));
        }
        Ok(path.to_string_lossy().to_string())
    }
}

/// 文件名净化：只留字母数字下划线（对应 Python `re.sub(r"[^A-Za-z0-9]+", "_", s)`）。
pub fn sanitize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_underscore = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "backup".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_matches_python_regex_shape() {
        assert_eq!(sanitize_name("HKLM\\SYSTEM\\X"), "HKLM_SYSTEM_X");
        assert_eq!(
            sanitize_name("a b  c"),
            "a_b_c",
            "连续非字母数字压成一个下划线"
        );
        assert_eq!(sanitize_name("dns_backup_WLAN"), "dns_backup_WLAN");
        assert_eq!(sanitize_name(""), "backup");
        assert_eq!(sanitize_name("///"), "backup");
        assert_eq!(sanitize_name("以太网"), "backup", "全非 ASCII 时回退占位名");
    }

    #[test]
    fn session_creates_timestamped_dir_under_repair_backups_root() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let s = BackupSession::new().expect("应能创建备份目录");
        let dir = s.dir();
        assert!(dir.exists(), "备份目录必须真实存在");
        assert!(
            dir.starts_with(paths::repair_backups_dir()),
            "必须落在 repair-backups 根目录下: {}",
            dir.display()
        );
        // 目录名是时间戳（15 字符：YYYYmmdd_HHMMSS）
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(name.len(), 15, "时间戳目录名: {name}");
        assert_eq!(name.chars().filter(|c| *c == '_').count(), 1);
    }

    #[test]
    fn save_note_writes_content_and_reports_path() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let s = BackupSession::new().unwrap();
        let p = s
            .save_note("dns_backup_WLAN.txt", "===== ipv4 =====\n223.5.5.5")
            .unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("223.5.5.5"));
        // 文件名按 Python `re.sub(r"[^A-Za-z0-9]+","_",s)` 净化：点号也变下划线
        assert!(p.ends_with("dns_backup_WLAN_txt"), "净化后的文件名: {p}");
    }

    #[test]
    fn save_note_sanitizes_unsafe_names() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let s = BackupSession::new().unwrap();
        let p = s.save_note("../../evil.txt", "x").unwrap();
        // 不允许逃出备份目录
        assert!(
            Path::new(&p).starts_with(s.dir()),
            "备份文件不得写到目录外: {p}"
        );
    }

    #[test]
    fn export_reg_round_trips_a_readable_key() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let s = BackupSession::new().unwrap();
        // NCSI 键普通用户可读，导出应成功并产出非空 .reg
        let p = s.export_reg(crate::netcheck::NCSI_REG_FULL);
        match p {
            Ok(path) => {
                let bytes = std::fs::read(&path).unwrap_or_default();
                assert!(!bytes.is_empty(), "备份文件不得为空");
                // reg export 默认写 UTF-16LE；用 lossy 解码才能读到 ASCII 头
                let text = String::from_utf16_lossy(
                    &bytes
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect::<Vec<u16>>(),
                );
                assert!(
                    text.contains("Registry Editor") || text.contains("Windows Registry"),
                    "应是 .reg 文本（UTF-16LE），前 80 字节: {:?}",
                    &bytes[..bytes.len().min(80)]
                );
            }
            Err(e) => {
                // 权限受限环境允许失败，但必须是 Err（→ 调用方中止动作），不是静默成功
                assert!(!e.is_empty());
            }
        }
    }

    #[test]
    fn export_reg_fails_loudly_for_bogus_path() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let s = BackupSession::new().unwrap();
        assert!(
            s.export_reg(r"HKLM\SYSTEM\NoSuchKey\SysKitDoesNotExist")
                .is_err(),
            "不存在的键必须返回 Err（否则动作会在无备份的情况下继续改）"
        );
    }
}
