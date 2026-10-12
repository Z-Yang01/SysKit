//! Hosts 文件编辑器逻辑：解析/序列化/增删改（对应 Python 侧 hostsed.py）。
//!
//! 条目结构：`kind` 为 `entry`（启用）/ `disabled`（被 `#` 注释掉的主机映射，
//! 可一键启用）/ `comment`（纯注释或空行，保留原文）。
//!
//! 两条**必须保留的护栏**（Python 注释里是用户数据损坏换来的）：
//! 1. **BOM**：`utf-8-sig` 读法——BOM 会污染首个 token，导致 IP 判定失败、
//!    整行被当成普通注释，用户的映射行就此"消失"；
//! 2. **写回前拒绝可疑编码**：文件里出现 `\x00`（UTF-16/二进制）或 U+FFFD
//!    （ANSI/GBK 中文注释被按 UTF-8 解出替换字符）时**拒绝写入**，
//!    否则保存会把注释永久写成问号——这是不可逆的数据损失。
//!
//! 序列化对未修改条目保留原始文本；主机映射行重建为规范行（原文件的制表符/
//! 对齐缩进不保留——语义不变，避免为保真引入逐字段 diff 复杂度）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::paths;

/// Windows hosts 文件默认路径。
pub const HOSTS_PATH: &str = r"C:\Windows\System32\drivers\etc\hosts";

/// 条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// 启用中的主机映射
    Entry,
    /// 被注释掉的主机映射（可一键启用）
    Disabled,
    /// 纯注释/空行（保留原文）
    Comment,
}

/// 一条 hosts 条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEntry {
    /// 原始行文本（仅注释/空行使用，用于逐字保留；映射行留空、由字段重建）
    pub raw: String,
    pub kind: EntryKind,
    pub ip: String,
    pub host: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub comment: String,
    pub enabled: bool,
    /// 原行是否有正文（`#` 单独一行时为 false）。`#[serde(default)]` 保证向后兼容。
    #[serde(default)]
    pub had_content: bool,
}

impl HostEntry {
    /// 是否为主机映射（启用或被注释的映射）。
    pub fn is_mapping(&self) -> bool {
        matches!(self.kind, EntryKind::Entry | EntryKind::Disabled)
    }
}

/// 首 token 是否是合法 IP（v4 或 v6）——只有这样才能视为主机映射行，
/// 避免把普通注释误判成"被禁用的条目"。
pub fn looks_like_entry(parts: &[&str]) -> bool {
    if parts.len() < 2 {
        return false;
    }
    parts[0].parse::<std::net::IpAddr>().is_ok()
}

/// hosts 文本 → 条目列表；畸形行保留原文、kind=comment。
///
/// 关于 `raw` 字段：**只在无法重建（畸形行）时才带原文**。
/// 映射行与可识别注释行的 raw 留空、由 `serialize` **惰性重建**——
/// 早期版本把原文塞进 raw，结果 `serialize(parse(text)) != text`：
/// 只要原行用两个空格对齐，往返就会被压成一个空格。惰性重建让
/// 「未修改即逐字保留」成立，与 Python 的 `kind == "comment"` 分支同效果。
pub fn parse(text: &str) -> Vec<HostEntry> {
    // 等价 Python `splitlines()`：**必须丢弃末尾换行产生的空元素**——
    // 否则每份以 \n 结尾的 hosts（几乎全部）都会多出一条空行，写回后文件末尾
    // 多一行，且条目下标全被这条幽灵行牵着走。注意只在「最后一段」上做这件事，
    // 文件**内部**的空行仍要保留成 comment 条目。
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    let mut entries = Vec::new();
    for raw in lines {
        // split('\n') 会留下 \r（CRLF 文件）；strip 时一并处理
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        // 等价 Python `stripped.lstrip("#").strip()`：去掉“行首（含前导空白）的
        // 全部 #”——这正是「禁用映射可一键启用」能识别多层注释的原因。
        let cmp = raw.trim_start();
        let hash_count = cmp.chars().take_while(|c| *c == '#').count();
        let disabled = hash_count > 0;
        let body = cmp[hash_count..].trim();
        // 行内注释
        let (main, inline) = match body.split_once('#') {
            Some((m, i)) => (m, i.trim()),
            None => (body, ""),
        };
        let parts: Vec<&str> = main.split_whitespace().collect();
        if looks_like_entry(&parts) {
            entries.push(HostEntry {
                raw: String::new(),
                kind: if disabled {
                    EntryKind::Disabled
                } else {
                    EntryKind::Entry
                },
                ip: parts[0].to_string(),
                host: parts[1].to_string(),
                aliases: parts[2..].iter().map(|s| s.to_string()).collect(),
                comment: inline.to_string(),
                enabled: !disabled,
                had_content: true,
            });
        } else {
            // 不是主机映射 → 注释行（含空行）。
            //
            // `comment` 取「剥掉的 # **之后**的原文」而非 `cmp[hash_count..]`：
            // serialize 会在前面补 `# `，若这里也带上 `#` 就会写出 `# # 注释`
            // （首轮移植时正是这么错的，round-trip 测试抓到的）。
            // 同时 raw 留空 → 该行也走惰性重建，保证 `serialize(parse(t)) == t`。
            entries.push(HostEntry {
                raw: raw.to_string(),
                kind: EntryKind::Comment,
                ip: String::new(),
                host: String::new(),
                aliases: Vec::new(),
                comment: body.to_string(),
                enabled: false,
                had_content: !body.is_empty(),
            });
        }
    }
    entries
}

/// 条目列表 → hosts 文本。
pub fn serialize(entries: &[HostEntry]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for e in entries {
        match e.kind {
            // 注释/空行：raw 非空（畸形行）用 raw，否则由 comment 重建 `# 文本`
            EntryKind::Comment => {
                if !e.raw.is_empty() {
                    lines.push(e.raw.clone());
                } else if e.comment.is_empty() {
                    lines.push(String::new());
                } else {
                    lines.push(format!("# {}", e.comment));
                }
            }
            _ => {
                if e.host.is_empty() {
                    lines.push(e.raw.clone());
                    continue;
                }
                let prefix = if e.enabled { "" } else { "# " };
                let mut parts = vec![e.ip.clone(), e.host.clone()];
                parts.extend(e.aliases.iter().cloned());
                let mut line = format!("{prefix}{}", parts.join(" "));
                if !e.comment.is_empty() {
                    line.push_str(&format!("    # {}", e.comment));
                }
                lines.push(line);
            }
        }
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

/// 按主机名（不区分大小写）查找下标；给定 ip 时需同时匹配。无则 None。
#[cfg(test)]
pub fn find_entry(entries: &[HostEntry], host: &str, ip: Option<&str>) -> Option<usize> {
    let host = host.to_lowercase();
    entries.iter().position(|e| {
        e.kind != EntryKind::Comment
            && e.host.to_lowercase() == host
            && ip.map(|want| e.ip == want).unwrap_or(true)
    })
}

/// 追加一条映射（纯内存）。
pub fn add_entry(
    entries: &mut Vec<HostEntry>,
    ip: &str,
    host: &str,
    comment: &str,
    aliases: &[String],
) {
    entries.push(HostEntry {
        raw: String::new(),
        kind: EntryKind::Entry,
        ip: ip.trim().to_string(),
        host: host.trim().to_string(),
        aliases: aliases
            .iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect(),
        comment: comment.trim().to_string(),
        enabled: true,
        had_content: true,
    });
}

/// 删除第 index 条；越界无操作，返回是否删除。
pub fn remove_entry(entries: &mut Vec<HostEntry>, index: usize) -> bool {
    if index < entries.len() {
        entries.remove(index);
        true
    } else {
        false
    }
}

/// 启用 ↔ 禁用（注释/取消注释）。comment 行无效果。
pub fn toggle_entry(entries: &mut [HostEntry], index: usize) -> bool {
    if index >= entries.len() {
        return false;
    }
    let e = &mut entries[index];
    if !e.is_mapping() {
        return false;
    }
    e.enabled = !e.enabled;
    e.kind = if e.enabled {
        EntryKind::Entry
    } else {
        EntryKind::Disabled
    };
    true
}

// ---------------- IO ----------------

/// 去 BOM（`utf-8-sig` 等价物）。
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// 读取 hosts 并解析；失败返回空表。
pub fn read_entries(path: Option<&Path>) -> Vec<HostEntry> {
    let p = path.unwrap_or_else(|| Path::new(HOSTS_PATH));
    match std::fs::read(p) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            parse(strip_bom(&text))
        }
        Err(_) => Vec::new(),
    }
}

/// 原始字节里是否含 NUL（UTF-16/二进制误读的判据）。
pub fn looks_binary_bytes(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

/// 写回前的编码护栏：返回 Err 表示**拒绝写入**。
///
/// 触发条件（任一）：
/// - 含 NUL：文件是 UTF-16/二进制，按 UTF-8 写回会彻底破坏；
/// - 含 U+FFFD：原文件里有非 UTF-8 字节（典型是 GBK 中文注释），
///   按 lossy 解码后写回会把注释永久变成问号。
pub fn encoding_guard(bytes: &[u8]) -> Result<(), String> {
    if looks_binary_bytes(bytes) {
        return Err(
            "检测到 hosts 文件编码异常（UTF-16/二进制），已阻止写入以保护原内容；\
             请先用记事本另存为 UTF-8。"
                .to_string(),
        );
    }
    let text = String::from_utf8_lossy(bytes);
    if text.contains('\u{fffd}') {
        return Err(
            "检测到 hosts 文件含非 UTF-8 字节（可能是 ANSI/GBK 中文注释），\
             已阻止写入以保护原内容；请先用记事本另存为 UTF-8。"
                .to_string(),
        );
    }
    Ok(())
}

/// 备份 hosts 到 `backup/hosts_<时间戳>/hosts`；源不存在返回 None。
pub fn backup_hosts(path: Option<&Path>) -> Result<Option<String>, String> {
    let p = path.unwrap_or_else(|| Path::new(HOSTS_PATH));
    if !p.exists() {
        return Ok(None);
    }
    let stamp = crate::timefmt::now_fmt("%Y%m%d_%H%M%S");
    let dir = paths::backup_dir().join(format!("hosts_{stamp}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("备份目录创建失败: {e}"))?;
    let dest = dir.join("hosts");
    std::fs::copy(p, &dest).map_err(|e| format!("备份拷贝失败: {e}"))?;
    Ok(Some(dest.to_string_lossy().to_string()))
}

/// 序列化并写回（临时文件 + 原子替换）。
///
/// **备份失败即拒绝写入**（安全侧：宁可不改）。返回备份路径。
pub fn write_entries(
    entries: &[HostEntry],
    path: Option<&Path>,
    backup: bool,
) -> Result<String, String> {
    let p = path.unwrap_or_else(|| Path::new(HOSTS_PATH));

    // 护栏：先看原文件字节，可疑编码一律拒绝
    if let Ok(bytes) = std::fs::read(p) {
        encoding_guard(&bytes)?;
    }

    // 文件不存在时没有任何东西需要备份 → 空路径（首次创建场景）
    let dest = if backup {
        backup_hosts(Some(p))?.unwrap_or_default()
    } else {
        String::new()
    };

    let text = serialize(entries);
    // newline="\n" 等价物：serialize 出的就是裸 \n；原子替换复用 stores 原子写
    crate::jsonio::atomic_write(p, text.as_bytes()).map_err(|e| format!("写回 hosts 失败: {e}"))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_file(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("syskit_hosts_{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("hosts")
    }

    #[test]
    fn parse_basic_entries_and_comments() {
        let text = "# 注释\n\n127.0.0.1  localhost\n10.0.0.1  dev.local  api.local    # 开发环境\n";
        let e = parse(text);
        assert_eq!(e.len(), 4);
        assert_eq!(e[0].kind, EntryKind::Comment);
        assert_eq!(e[1].kind, EntryKind::Comment, "空行也是 comment");
        assert_eq!(e[2].kind, EntryKind::Entry);
        assert_eq!(e[2].ip, "127.0.0.1");
        assert_eq!(e[2].host, "localhost");
        assert!(e[2].enabled);
        assert_eq!(e[3].host, "dev.local");
        assert_eq!(e[3].aliases, vec!["api.local"]);
        assert_eq!(e[3].comment, "开发环境", "行内注释要取出来");
    }

    #[test]
    fn parse_disabled_mapping_is_recognized() {
        let e = parse("# 10.0.0.2  blocked.local\n");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].kind, EntryKind::Disabled, "被注释的映射要能识别");
        assert_eq!(e[0].ip, "10.0.0.2");
        assert_eq!(e[0].host, "blocked.local");
        assert!(!e[0].enabled);
    }

    #[test]
    fn parse_plain_comment_is_not_mistaken_for_disabled_entry() {
        // 首 token 不是 IP → 只能是普通注释（这是 `_looks_like_entry` 的全部意义）
        let e = parse("# 这是说明文字 不是映射\n");
        assert_eq!(e[0].kind, EntryKind::Comment);
        assert_eq!(e[0].comment, "这是说明文字 不是映射");
        let e2 = parse("# 只一个词\n");
        assert_eq!(e2[0].kind, EntryKind::Comment, "单 token 不够成映射");
    }

    #[test]
    fn parse_accepts_ipv6() {
        let e = parse("::1  localhost\nfe80::1  router.local\n");
        assert_eq!(e[0].kind, EntryKind::Entry);
        assert_eq!(e[0].ip, "::1");
        assert_eq!(e[1].ip, "fe80::1");
    }

    #[test]
    fn parse_handles_crlf_and_bom() {
        let text = "\u{feff}127.0.0.1  localhost\r\n10.0.0.1  x.local\r\n";
        let e = parse(strip_bom(text));
        assert_eq!(e.len(), 2);
        assert_eq!(
            e[0].kind,
            EntryKind::Entry,
            "BOM 必须先剥掉，否则首个 token 不是合法 IP"
        );
        assert_eq!(e[0].ip, "127.0.0.1");
        assert_eq!(e[1].host, "x.local", "CRLF 的 \\r 不能留在 host 里");
    }

    #[test]
    fn serialize_keeps_plain_comments_and_blank_lines_verbatim() {
        // 契约（与 Python 相同）：**纯注释行与空行逐字保留**。
        // 注意「被注释掉的映射」不属于此列——它的正文会被重建（见下一个用例）。
        let text = "# 头部注释\n\n# 需要保留的说明文字\n#\n";
        let out = serialize(&parse(text));
        assert_eq!(out, text, "纯注释/空行必须逐字保留");
    }

    #[test]
    fn serialize_normalizes_mapping_lines_including_disabled_ones() {
        // 契约（与 Python 相同）：**映射行的正文会重建为规范单空格形态**，
        // 启用与被注释掉的都一样（`# 10.0.0.2  被禁用的映射` → 单空格）。
        // 原文件的制表符/对齐缩进不保留（语义不变，换掉逐字段 diff 的复杂度）。
        // 明确写出来，免得后人把「对齐被压掉」当成 bug。
        let enabled = "127.0.0.1  localhost\n";
        assert_eq!(serialize(&parse(enabled)), "127.0.0.1 localhost\n");

        let disabled = "# 10.0.0.2  被禁用的映射\n";
        let out = serialize(&parse(disabled));
        assert_eq!(out, "# 10.0.0.2 被禁用的映射\n");
        // 但关键信息一条不丢
        let e = parse(disabled);
        assert_eq!(e[0].kind, EntryKind::Disabled);
        assert_eq!(e[0].ip, "10.0.0.2");
        assert_eq!(e[0].host, "被禁用的映射");
    }

    #[test]
    fn serialize_disabled_uses_hash_prefix() {
        let mut e = parse("10.0.0.1  x.local\n");
        toggle_entry(&mut e, 0);
        let out = serialize(&e);
        assert_eq!(out, "# 10.0.0.1 x.local\n");
        // 再切回来
        toggle_entry(&mut e, 0);
        assert_eq!(serialize(&e), "10.0.0.1 x.local\n");
    }

    #[test]
    fn serialize_rebuilds_mapping_line_with_aliases_and_comment() {
        let mut e = parse("# 注释\n");
        add_entry(
            &mut e,
            "10.1.1.1",
            "a.local",
            "备注",
            &["b.local".into(), "".into()],
        );
        let out = serialize(&e);
        assert!(out.contains("10.1.1.1 a.local b.local    # 备注"), "{out}");
        assert!(out.starts_with("# 注释"), "原注释行仍在");
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn serialize_empty_list_is_empty_string() {
        assert_eq!(serialize(&[]), "");
    }

    #[test]
    fn find_entry_case_insensitive_and_ip_aware() {
        let e = parse("10.0.0.1  Dev.Local\n");
        assert_eq!(find_entry(&e, "dev.local", None), Some(0));
        assert_eq!(find_entry(&e, "DEV.LOCAL", None), Some(0));
        assert_eq!(find_entry(&e, "dev.local", Some("10.0.0.1")), Some(0));
        assert_eq!(
            find_entry(&e, "dev.local", Some("10.0.0.2")),
            None,
            "ip 不匹配则找不到"
        );
        assert_eq!(find_entry(&e, "nope.local", None), None);
    }

    #[test]
    fn find_entry_skips_comments() {
        let e = parse("# dev.local 只是注释\n");
        assert_eq!(find_entry(&e, "dev.local", None), None);
    }

    #[test]
    fn toggle_has_no_effect_on_comment_rows() {
        let mut e = parse("# 纯注释\n");
        assert!(!toggle_entry(&mut e, 0));
        assert_eq!(e[0].kind, EntryKind::Comment);
        assert!(!toggle_entry(&mut e, 99), "越界返回 false");
    }

    #[test]
    fn remove_entry_bounds_checked() {
        let mut e = parse("10.0.0.1  a\n10.0.0.2  b\n");
        assert!(remove_entry(&mut e, 0));
        assert_eq!(e.len(), 1);
        assert!(!remove_entry(&mut e, 5));
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn add_entry_trims_and_drops_empty_aliases() {
        let mut e = Vec::new();
        add_entry(
            &mut e,
            "  10.0.0.1  ",
            "  x.local ",
            " 备注 ",
            &["  ".into(), "y.local".into()],
        );
        assert_eq!(e[0].ip, "10.0.0.1");
        assert_eq!(e[0].host, "x.local");
        assert_eq!(e[0].comment, "备注");
        assert_eq!(e[0].aliases, vec!["y.local"]);
    }

    // ---------------- 编码护栏 ----------------

    #[test]
    fn guard_rejects_nul_bytes() {
        let utf16 = "127.0.0.1  localhost"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect::<Vec<u8>>();
        let err = encoding_guard(&utf16).unwrap_err();
        assert!(err.contains("UTF-16"), "{err}");
    }

    #[test]
    fn guard_rejects_invalid_utf8_like_gbk_comments() {
        // GBK 编码的「中文」不是合法 UTF-8 → lossy 解码出 U+FFFD
        let mut bytes = b"127.0.0.1  localhost  # ".to_vec();
        bytes.extend_from_slice(&[0xD6, 0xD0, 0xCE, 0xC4]); // GBK "中文"
        let err = encoding_guard(&bytes).unwrap_err();
        assert!(err.contains("非 UTF-8"), "{err}");
    }

    #[test]
    fn guard_accepts_clean_ascii_and_utf8() {
        assert!(encoding_guard(b"127.0.0.1  localhost\n").is_ok());
        assert!(encoding_guard("10.0.0.1  x  # 中文注释\n".as_bytes()).is_ok());
        // 带 BOM 的 UTF-8 也合法
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(b"127.0.0.1 localhost\n");
        assert!(encoding_guard(&with_bom).is_ok());
    }

    #[test]
    fn read_entries_missing_file_is_empty() {
        let p = temp_file("missing").with_file_name("nope_hosts");
        assert!(read_entries(Some(&p)).is_empty());
    }

    #[test]
    fn write_backs_up_and_round_trips() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let p = temp_file("rt");
        std::fs::write(&p, "127.0.0.1  localhost\n").unwrap();

        let mut e = read_entries(Some(&p));
        assert_eq!(e.len(), 1);
        add_entry(&mut e, "10.9.9.9", "new.local", "测试", &[]);
        let backup = write_entries(&e, Some(&p), true).expect("写入应成功");
        assert!(!backup.is_empty(), "必须产生备份路径");
        assert!(std::path::Path::new(&backup).is_file(), "备份文件必须存在");

        let after = read_entries(Some(&p));
        assert_eq!(after.len(), 2);
        assert_eq!(after[1].host, "new.local");
        // 原子写已收敛到 stores：残留检查对齐其 <path>.tmp 约定
        assert!(
            !std::path::PathBuf::from(format!("{}.tmp", p.display())).exists(),
            "临时文件应已被替换掉"
        );
    }

    #[test]
    fn write_refuses_when_encoding_suspicious() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let p = temp_file("guard");
        // UTF-16 hosts：写回会彻底破坏，必须拒绝
        let utf16: Vec<u8> = "127.0.0.1 localhost"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        std::fs::write(&p, &utf16).unwrap();
        let e = parse("127.0.0.1  localhost\n");
        let err = write_entries(&e, Some(&p), true).unwrap_err();
        assert!(
            err.contains("编码异常") || err.contains("非 UTF-8"),
            "{err}"
        );
        // 原文件必须**未被改动**
        assert_eq!(
            std::fs::read(&p).unwrap(),
            utf16,
            "拒绝写入时原文件不得变动"
        );
    }

    #[test]
    fn write_creates_file_when_absent() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let p = temp_file("create").with_file_name("brand_new_hosts");
        let e = parse("127.0.0.1  localhost\n");
        let backup = write_entries(&e, Some(&p), true).expect("首次创建应允许");
        assert!(backup.is_empty(), "原文件不存在 → 无备份");
        assert!(p.is_file());
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "127.0.0.1 localhost\n"
        );
    }
}
