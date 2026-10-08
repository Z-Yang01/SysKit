//! 密码库存储（M6 数据层）：SQLite sites/accounts 两级模型。
//!
//! 逐语义移植 `syskit/vault.py`：SQL 逐字对齐（同一 vault.db 必须两版互开——
//! 跨语言硬约束，文本列必须存 TEXT 而非 BLOB）；搜索只走明文元数据，
//! 密码/TOTP/备注永远密文落盘、按需解密；加密由调用方传入的
//! [`VaultCrypto`](crate::vault_crypto::VaultCrypto) 承担。
//! 与 Python 的差异仅实现层：icon 列不出存储层（Python dict 携带但 UI 从未消费）。

use std::path::PathBuf;

use rusqlite::Connection;

use crate::timefmt;
use crate::vault_crypto::VaultCrypto;

/// 与 Python `vault.SCHEMA` 逐字一致——schema 分叉 = 同库互开直接破裂。
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sites (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  domain TEXT DEFAULT '',
  url TEXT DEFAULT '',
  aliases TEXT DEFAULT '',
  tags TEXT DEFAULT '',
  icon BLOB,
  created_at TEXT,
  updated_at TEXT
);
CREATE TABLE IF NOT EXISTS accounts (
  id INTEGER PRIMARY KEY,
  site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
  label TEXT DEFAULT '',
  username TEXT DEFAULT '',
  password_enc BLOB NOT NULL,
  totp_enc BLOB,
  notes_enc BLOB,
  is_favorite INTEGER DEFAULT 0,
  last_used_at TEXT,
  created_at TEXT,
  updated_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_accounts_site ON accounts(site_id, is_favorite DESC);
CREATE INDEX IF NOT EXISTS idx_sites_name ON sites(name COLLATE NOCASE);
";

/// SCHEMA 结构变更时递增并补迁移（当前纯 IF-NOT-EXISTS 幂等 DDL）。
const SCHEMA_VERSION: i32 = 1;

/// 测试专用 DB 目录覆盖（Mutex 可重设——paths::set_appdata_override 是
/// OnceLock 一次性的，跨语言 fixture 测试需要来回指，实测踩过）。
#[cfg(test)]
static DB_OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn set_db_override(dir: std::path::PathBuf) {
    *DB_OVERRIDE.lock().expect("db override 锁中毒") = Some(dir);
}

#[cfg(test)]
fn clear_db_override() {
    *DB_OVERRIDE.lock().expect("db override 锁中毒") = None;
}

fn db_path() -> PathBuf {
    #[cfg(test)]
    if let Some(dir) = DB_OVERRIDE.lock().expect("db override 锁中毒").as_ref() {
        return dir.join("vault.db");
    }
    crate::paths::appdata_dir().join("vault.db")
}

fn now() -> String {
    timefmt::now_fmt("%Y-%m-%dT%H:%M:%S")
}

fn conn() -> rusqlite::Result<Connection> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent); // 便携首启：data\config 尚不存在
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA foreign_keys = ON")?;
    Ok(conn)
}

/// init_db 的进程级短路标志（首次成功后不再重复 open+PRAGMA）。
static DB_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 建库/补齐 schema：user_version 已达即跳过（免每启动重放 DDL）。
///
/// 进程级短路：首次成功后直接返回（每次 open+PRAGMA 是一次文件 IO，
/// vault 命令走 session_lock 时每条都会经过这里）。失败不置位，下条命令重试。
pub fn init_db() -> rusqlite::Result<()> {
    if DB_READY.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    let conn = conn()?;
    let ver: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if ver < SCHEMA_VERSION {
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    }
    DB_READY.store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}

/// 测试专用：隔离目录换新后清进程级短路标志（生产代码不得调用）。
#[cfg(test)]
pub fn reset_db_ready_for_test() {
    DB_READY.store(false, std::sync::atomic::Ordering::Release);
}

// ---------------- sites ----------------

/// 与 Python `dict(row)` 键名对齐（icon 不出存储层，见模块注释）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SiteRow {
    pub id: i64,
    pub name: String,
    pub domain: String,
    pub url: String,
    pub aliases: String,
    pub tags: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub n_accounts: i64,
}

pub fn add_site(
    name: &str,
    domain: &str,
    url: &str,
    aliases: &str,
    tags: &str,
) -> rusqlite::Result<i64> {
    let conn = conn()?;
    conn.execute(
        "INSERT INTO sites(name, domain, url, aliases, tags, created_at, updated_at) VALUES(?,?,?,?,?,?,?)",
        rusqlite::params![name, domain, url, aliases, tags, now(), now()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 站点可编辑字段（None=不改），对齐 Python update_site 白名单。
#[derive(Debug, Default, Clone)]
pub struct SitePatch {
    pub name: Option<String>,
    pub domain: Option<String>,
    pub url: Option<String>,
    pub aliases: Option<String>,
    pub tags: Option<String>,
}

pub fn update_site(site_id: i64, fields: &SitePatch) -> rusqlite::Result<()> {
    let mut sets = Vec::new();
    let mut vals: Vec<rusqlite::types::Value> = Vec::new();
    for (col, v) in [
        ("name", &fields.name),
        ("domain", &fields.domain),
        ("url", &fields.url),
        ("aliases", &fields.aliases),
        ("tags", &fields.tags),
    ] {
        if let Some(v) = v {
            sets.push(format!("{col}=?"));
            vals.push(rusqlite::types::Value::Text(v.clone()));
        }
    }
    if sets.is_empty() {
        return Ok(());
    }
    sets.push("updated_at=?".into());
    vals.push(rusqlite::types::Value::Text(now()));
    vals.push(rusqlite::types::Value::Integer(site_id));
    let sql = format!("UPDATE sites SET {} WHERE id=?", sets.join(","));
    let conn = conn()?;
    let params: Vec<&dyn rusqlite::types::ToSql> = vals
        .iter()
        .map(|v| v as &dyn rusqlite::types::ToSql)
        .collect();
    conn.execute(&sql, params.as_slice())?;
    Ok(())
}

pub fn delete_site(site_id: i64) -> rusqlite::Result<()> {
    // 手动先删 accounts 再删 sites：对齐 Python（未依赖 FK CASCADE，语义等价）
    let conn = conn()?;
    conn.execute("DELETE FROM accounts WHERE site_id=?", [site_id])?;
    conn.execute("DELETE FROM sites WHERE id=?", [site_id])?;
    Ok(())
}

fn map_site(r: &rusqlite::Row) -> rusqlite::Result<SiteRow> {
    Ok(SiteRow {
        id: r.get("id")?,
        name: r.get("name")?,
        domain: r.get::<_, Option<String>>("domain")?.unwrap_or_default(),
        url: r.get::<_, Option<String>>("url")?.unwrap_or_default(),
        aliases: r.get::<_, Option<String>>("aliases")?.unwrap_or_default(),
        tags: r.get::<_, Option<String>>("tags")?.unwrap_or_default(),
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        n_accounts: 0,
    })
}

fn site_rows(
    conn: &Connection,
    extra_where: &str,
    params: &[&dyn rusqlite::types::ToSql],
) -> rusqlite::Result<Vec<SiteRow>> {
    let sql = format!("SELECT s.*, (SELECT COUNT(*) FROM accounts a WHERE a.site_id=s.id) AS n_accounts FROM sites s {extra_where} ORDER BY s.name COLLATE NOCASE");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params, |r| {
        let mut s = map_site(r)?;
        s.n_accounts = r.get("n_accounts")?;
        Ok(s)
    })?;
    rows.collect()
}

#[allow(dead_code)] // M6 数据层 API 面（站点列表聚合由 search/get_secrets_bulk 承担）
pub fn list_sites() -> rusqlite::Result<Vec<SiteRow>> {
    let conn = conn()?;
    site_rows(&conn, "", &[])
}

/// LIKE 通配符转义（配 `ESCAPE '\'`）：用户输入的 % / _ 按字面匹配。
fn escape_like(q: &str) -> String {
    q.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub fn search_sites(q: &str) -> rusqlite::Result<Vec<SiteRow>> {
    let q = escape_like(q.trim().to_lowercase().as_str());
    let conn = conn()?;
    if q.is_empty() {
        return site_rows(&conn, "", &[]);
    }
    let like = format!("%{q}%");
    site_rows(
        &conn,
        "WHERE lower(name) LIKE ?1 ESCAPE '\\' OR lower(coalesce(domain,'')) LIKE ?1 ESCAPE '\\' OR lower(coalesce(url,'')) LIKE ?1 ESCAPE '\\' OR lower(coalesce(aliases,'')) LIKE ?1 ESCAPE '\\' OR lower(coalesce(tags,'')) LIKE ?1 ESCAPE '\\'",
        &[&like],
    )
}

/// 按域名精确匹配（自动填充预留边界）。别名「逗号分词精确相等」，
/// 拒绝子串——子串会让 mail.com 命中 gmail.com 别名（凭据授错域）。
#[allow(dead_code)] // M6 数据层 API 面（站点聚合由 search/get_secrets_bulk 承担）
pub fn find_by_domain(domain: &str) -> rusqlite::Result<Option<SiteRow>> {
    let domain = domain.trim().to_lowercase();
    if domain.is_empty() {
        return Ok(None);
    }
    let conn = conn()?;
    let mut stmt = conn.prepare("SELECT * FROM sites WHERE lower(domain)=?1 OR lower(url)=?2")?;
    let mut rows = stmt.query_map(
        rusqlite::params![domain, format!("https://{domain}")],
        map_site,
    )?;
    if let Some(first) = rows.next() {
        return Ok(Some(first?));
    }
    let mut stmt2 = conn.prepare("SELECT * FROM sites WHERE lower(coalesce(aliases,'')) <> ''")?;
    let rows = stmt2.query_map([], map_site)?;
    for r in rows {
        let site = r?;
        let toks: Vec<String> = site
            .aliases
            .split([',', '，', ';', '；'])
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if toks.contains(&domain) {
            return Ok(Some(site));
        }
    }
    Ok(None)
}

#[allow(dead_code)] // 同上
pub fn get_site(site_id: i64) -> rusqlite::Result<Option<SiteRow>> {
    let conn = conn()?;
    let mut stmt = conn.prepare("SELECT * FROM sites WHERE id=?")?;
    let mut rows = stmt.query_map([site_id], map_site)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

// ---------------- accounts ----------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountRow {
    pub id: i64,
    pub site_id: i64,
    pub label: String,
    pub username: String,
    pub is_favorite: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    pub has_totp: bool,
    pub has_notes: bool,
}

/// 账号元数据列表（不含任何明文 secrets）；置顶在前、用户名 NOCASE 次之。
pub fn list_accounts(site_id: i64, q: &str) -> rusqlite::Result<Vec<AccountRow>> {
    let q = escape_like(q.trim().to_lowercase().as_str());
    let conn = conn()?;
    let (cond, like) = if q.is_empty() {
        ("WHERE a.site_id=?1".to_string(), String::new())
    } else {
        (
            "WHERE a.site_id=?1 AND (lower(coalesce(a.username,'')) LIKE ?2 ESCAPE '\\' OR lower(coalesce(a.label,'')) LIKE ?2 ESCAPE '\\')"
                .to_string(),
            format!("%{q}%"),
        )
    };
    let sql = format!("SELECT a.id, a.site_id, a.label, a.username, a.is_favorite, a.last_used_at, a.totp_enc IS NOT NULL AS has_totp, a.notes_enc IS NOT NULL AS has_notes FROM accounts a {cond} ORDER BY a.is_favorite DESC, a.username COLLATE NOCASE");
    let mut stmt = conn.prepare(&sql)?;
    let map = |r: &rusqlite::Row| -> rusqlite::Result<AccountRow> {
        Ok(AccountRow {
            id: r.get("id")?,
            site_id: r.get("site_id")?,
            label: r.get::<_, Option<String>>("label")?.unwrap_or_default(),
            username: r.get::<_, Option<String>>("username")?.unwrap_or_default(),
            is_favorite: r.get::<_, i64>("is_favorite")? != 0,
            last_used_at: r.get("last_used_at")?,
            has_totp: r.get::<_, i64>("has_totp")? != 0,
            has_notes: r.get::<_, i64>("has_notes")? != 0,
        })
    };
    if q.is_empty() {
        let rows = stmt.query_map([site_id], map)?;
        rows.collect()
    } else {
        let rows = stmt.query_map(rusqlite::params![site_id, like], map)?;
        rows.collect()
    }
}

fn field_col(field: &str) -> Option<&'static str> {
    match field {
        "password" => Some("password_enc"),
        "totp" => Some("totp_enc"),
        "notes" => Some("notes_enc"),
        _ => None,
    }
}

/// 新增账号。密码必须非空（Python ValueError 语义 → Err）；空 totp/notes 存 NULL。
pub fn add_account(
    site_id: i64,
    password: &str,
    crypto: &VaultCrypto,
    label: &str,
    username: &str,
    totp_secret: &str,
    notes: &str,
) -> Result<i64, String> {
    if password.is_empty() {
        return Err("密码不能为空".into());
    }
    let now = now();
    let enc = |s: &str| crypto.encrypt(s.as_bytes()).map_err(|e| e.to_string());
    let password_enc = enc(password)?;
    let totp_enc = if totp_secret.is_empty() {
        None
    } else {
        Some(enc(totp_secret)?)
    };
    let notes_enc = if notes.is_empty() {
        None
    } else {
        Some(enc(notes)?)
    };
    let conn = conn().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO accounts(site_id, label, username, password_enc, totp_enc, notes_enc, created_at, updated_at) VALUES(?,?,?,?,?,?,?,?)",
        rusqlite::params![
            site_id, label, username, password_enc, totp_enc, notes_enc, now, now
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// 更新账号：None=不改；空串密码=不改（Python `if password:`）；空 totp/notes 置 NULL。
#[derive(Debug, Default, Clone)]
pub struct AccountPatch {
    pub label: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub totp_secret: Option<String>,
    pub notes: Option<String>,
}

pub fn update_account(
    account_id: i64,
    crypto: &VaultCrypto,
    patch: &AccountPatch,
) -> Result<(), String> {
    let mut sets: Vec<String> = Vec::new();
    let mut vals: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(v) = &patch.label {
        sets.push("label=?".into());
        vals.push(rusqlite::types::Value::Text(v.clone()));
    }
    if let Some(v) = &patch.username {
        sets.push("username=?".into());
        vals.push(rusqlite::types::Value::Text(v.clone()));
    }
    if let Some(p) = &patch.password {
        // 空串密码=不改（Python `if password:`）
        if !p.is_empty() {
            sets.push("password_enc=?".into());
            vals.push(rusqlite::types::Value::Blob(
                crypto.encrypt(p.as_bytes()).map_err(|e| e.to_string())?,
            ));
        }
    }
    if let Some(t) = &patch.totp_secret {
        sets.push("totp_enc=?".into());
        vals.push(if t.is_empty() {
            rusqlite::types::Value::Null
        } else {
            rusqlite::types::Value::Blob(crypto.encrypt(t.as_bytes()).map_err(|e| e.to_string())?)
        });
    }
    if let Some(n) = &patch.notes {
        sets.push("notes_enc=?".into());
        vals.push(if n.is_empty() {
            rusqlite::types::Value::Null
        } else {
            rusqlite::types::Value::Blob(crypto.encrypt(n.as_bytes()).map_err(|e| e.to_string())?)
        });
    }
    if sets.is_empty() {
        return Ok(());
    }
    sets.push("updated_at=?".into());
    vals.push(rusqlite::types::Value::Text(now()));
    vals.push(rusqlite::types::Value::Integer(account_id));
    let sql = format!("UPDATE accounts SET {} WHERE id=?", sets.join(","));
    let conn = conn().map_err(|e| e.to_string())?;
    let params: Vec<&dyn rusqlite::types::ToSql> = vals
        .iter()
        .map(|v| v as &dyn rusqlite::types::ToSql)
        .collect();
    conn.execute(&sql, params.as_slice())
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn delete_account(account_id: i64) -> rusqlite::Result<()> {
    let conn = conn()?;
    conn.execute("DELETE FROM accounts WHERE id=?", [account_id])?;
    Ok(())
}

pub fn toggle_favorite(account_id: i64) -> rusqlite::Result<bool> {
    let conn = conn()?;
    let n = conn.execute(
        "UPDATE accounts SET is_favorite = 1 - is_favorite WHERE id=?",
        [account_id],
    )?;
    Ok(n > 0)
}

pub fn mark_used(account_id: i64) -> rusqlite::Result<()> {
    let conn = conn()?;
    conn.execute(
        "UPDATE accounts SET last_used_at=? WHERE id=?",
        rusqlite::params![now(), account_id],
    )?;
    Ok(())
}

/// 按需解密单字段（field ∈ password|totp|notes）。touch=复制语义（顺带 mark_used）。
/// 缺密文返回空串；解密失败（含锁定态）向上抛——调用方给用户可见的降级提示。
pub fn get_secret(
    account_id: i64,
    crypto: &VaultCrypto,
    field: &str,
    touch: bool,
) -> Result<String, String> {
    let col = field_col(field).ok_or_else(|| format!("未知字段 {field}"))?;
    let conn = conn().map_err(|e| e.to_string())?;
    let sql = format!("SELECT {col} FROM accounts WHERE id=?");
    let blob: Option<Vec<u8>> = conn
        .query_row(&sql, [account_id], |r| r.get::<_, Option<Vec<u8>>>(0))
        .map_err(|e| e.to_string())?;
    let Some(blob) = blob else {
        return Ok(String::new());
    };
    if touch {
        mark_used(account_id).map_err(|e| e.to_string())?;
    }
    let plain = crypto.decrypt(&blob).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&plain).into_owned())
}

/// 批量解密（显示语义，不更新 last_used_at）。
/// 契约（补55 C-5 平移）：任何单条解密失败——含锁定态——都降级为该条缺席，
/// 不外溢；调用方按缺席显示 **** 即可。
pub fn get_secrets_bulk(
    account_ids: &[i64],
    crypto: &VaultCrypto,
    field: &str,
) -> Result<std::collections::HashMap<i64, String>, String> {
    let col = field_col(field).ok_or_else(|| format!("未知字段 {field}"))?;
    let mut ids: Vec<i64> = account_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(Default::default());
    }
    let marks = vec!["?"; ids.len()].join(",");
    let sql = format!("SELECT id, {col} FROM accounts WHERE id IN ({marks})");
    let conn = conn().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut out = std::collections::HashMap::new();
    for row in rows {
        let (id, blob) = row.map_err(|e| e.to_string())?;
        let Some(blob) = blob else { continue };
        if let Ok(plain) = crypto.decrypt(&blob) {
            out.insert(id, String::from_utf8_lossy(&plain).into_owned());
        }
    }
    Ok(out)
}

/// 全账号行（重复密码检测专用）。明文不出调用方比对聚合，不落日志。
pub type AccountPasswordRow = (i64, String, String, Option<Vec<u8>>);

/// 全账号行读取（重复密码检测专用）：JOIN sites 取站名；`password_enc` 可为
/// NULL（未设密码），由调用方跳过。
pub fn all_accounts_with_passwords() -> rusqlite::Result<Vec<AccountPasswordRow>> {
    let conn = conn()?;
    let mut stmt = conn.prepare(
        "SELECT a.id, coalesce(s.name,''), coalesce(a.label,''), a.password_enc          FROM accounts a JOIN sites s ON s.id=a.site_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            r.get::<_, Option<Vec<u8>>>(3)?,
        ))
    })?;
    rows.collect()
}

/// 全局搜索：空格分词 AND，命中网站名/域名/别名/标签/用户名/label 的拼接串。
/// 只返回元数据，不解密。
pub fn global_search(q: &str) -> rusqlite::Result<Vec<serde_json::Value>> {
    let tokens: Vec<String> = q
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Ok(vec![]);
    }
    let conn = conn()?;
    let mut stmt = conn.prepare("SELECT a.id, a.site_id, a.label, a.username, a.is_favorite, a.totp_enc IS NOT NULL AS has_totp, s.name AS site_name, s.domain AS site_domain, coalesce(s.aliases,'') AS site_aliases, coalesce(s.tags,'') AS site_tags FROM accounts a JOIN sites s ON s.id=a.site_id")?;
    let rows = stmt.query_map([], |r| {
        Ok(serde_json::json!({
            "id": r.get::<_, i64>("id")?,
            "site_id": r.get::<_, i64>("site_id")?,
            "label": r.get::<_, Option<String>>("label")?.unwrap_or_default(),
            "username": r.get::<_, Option<String>>("username")?.unwrap_or_default(),
            "is_favorite": r.get::<_, i64>("is_favorite")? != 0,
            "has_totp": r.get::<_, i64>("has_totp")? != 0,
            "site_name": r.get::<_, Option<String>>("site_name")?.unwrap_or_default(),
            "site_domain": r.get::<_, Option<String>>("site_domain")?.unwrap_or_default(),
            "site_aliases": r.get::<_, Option<String>>("site_aliases")?.unwrap_or_default(),
            "site_tags": r.get::<_, Option<String>>("site_tags")?.unwrap_or_default(),
        }))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let d = row?;
        let hay = format!(
            "{} {} {} {} {} {}",
            d["site_name"].as_str().unwrap_or(""),
            d["site_domain"].as_str().unwrap_or(""),
            d["site_aliases"].as_str().unwrap_or(""),
            d["site_tags"].as_str().unwrap_or(""),
            d["username"].as_str().unwrap_or(""),
            d["label"].as_str().unwrap_or(""),
        )
        .to_lowercase();
        if tokens.iter().all(|t| hay.contains(t.as_str())) {
            out.push(d);
        }
    }
    Ok(out)
}

// ---------------- TOTP（RFC 6238） ----------------

/// RFC 4648 base32 解码（A-Z2-7，调用侧已容 `=` padding）。
fn b32_decode(clean: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut buf = Vec::with_capacity(clean.len() * 5 / 8);
    let mut acc: u64 = 0;
    let mut bits = 0u32;
    for c in clean.bytes() {
        let v = ALPHABET.iter().position(|a| *a == c)? as u32;
        acc = (acc << 5) | u64::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            buf.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Some(buf)
}

/// Base32 密钥 → TOTP 验证码。空密钥/含字母表外字符（0/1/8/9 等）返回 ""
/// （录入错误不抛异常——Python 同语义，防 UI 槽静默失效重演）。
/// 兼容「已带 = padding」「含空格/连字符」的常见粘贴格式。
pub fn totp_code(secret_b32: &str, at: Option<f64>, digits: usize, period: u64) -> String {
    let clean: String = secret_b32
        .to_uppercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '=')
        .collect();
    if clean.is_empty() {
        return String::new();
    }
    if !clean
        .chars()
        .all(|c| c.is_ascii_uppercase() || ('2'..='7').contains(&c))
    {
        return String::new();
    }
    let Some(key) = b32_decode(&clean) else {
        return String::new();
    };
    let t = at.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    });
    let counter = (t / period as f64).floor() as u64;
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(&key).expect("HMAC 接受任意长度密钥");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let d = &digest[..];
    let offset = (d[d.len() - 1] & 0x0F) as usize;
    let code = (u32::from_be_bytes([d[offset], d[offset + 1], d[offset + 2], d[offset + 3]])
        & 0x7FFF_FFFF)
        % 10u32.pow(digits as u32);
    format!("{code:0digits$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_rfc6238_vectors() {
        // RFC 6238 附录 B（SHA1，secret = ASCII "12345678901234567890"）
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(totp_code(secret, Some(59.0), 8, 30), "94287082");
        assert_eq!(totp_code(secret, Some(1111111109.0), 8, 30), "07081804");
        assert_eq!(totp_code(secret, Some(1234567890.0), 8, 30), "89005924");
        // 6 位 = 同一 31 位截断值 % 10^6
        assert_eq!(totp_code(secret, Some(59.0), 6, 30), "287082");
    }

    #[test]
    fn totp_rejects_bad_secret_like_python() {
        assert_eq!(totp_code("", None, 6, 30), "");
        // 0/1/8/9 不在 base32 字母表：录入错误返回空而非抛异常
        assert_eq!(totp_code("JB1WY3DP", None, 6, 30), "");
        // 空格/连字符/= padding 容忍
        assert!(!totp_code("jbsw y3dp-ehpk=3pxp=", None, 6, 30).is_empty());
    }

    /// 前置：调用方必须已持 `serial()` 令牌并调用过 `isolate()`——
    /// 本函数不再加锁（std Mutex 不可重入，嵌套加锁会死锁，实测踩过）。
    /// DPAPI 模式下密钥文件已存在时需显式解锁载入数据密钥。
    fn crypto_dpapi() -> VaultCrypto {
        let mut c = VaultCrypto::new(); // 默认路径随 override 隔离
        if !c.is_provisioned() {
            c.setup(None).expect("dpapi 初始化"); // None = DPAPI 模式
            return c;
        }
        // 文件已在：大概率本测试此前跑过留下的 DPAPI 密钥，直接解锁。
        // 但共享隔离目录里也可能是**其他测试**留下的主密码模式残件——
        // unlock(None) 解不开就删掉重建（顺序依赖因此消失；真机实锤
        // 2026-10-04：全量套件顺序下 bulk_decrypt/crud 撞残留密钥报
        // "wrapped_key 缺失"，单跑却全绿）。
        if c.unlock(None).is_ok() {
            return c;
        }
        let _ = std::fs::remove_file(c.path());
        let mut fresh = VaultCrypto::new();
        fresh.setup(None).expect("dpapi 残件清理后重建");
        fresh
    }

    #[test]
    fn site_and_account_crud_roundtrip() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        reset_db_ready_for_test();
        init_db().unwrap();
        let crypto = crypto_dpapi();
        let site = add_site("GitHub", "github.com", "https://github.com", "gh", "代码").unwrap();
        assert_eq!(get_site(site).unwrap().unwrap().name, "GitHub");

        let acc = add_account(
            site,
            "pw-密码",
            &crypto,
            "主账号",
            "alice",
            "JBSWY3DPEHPK3PXP",
            "备注",
        )
        .unwrap();
        let accs = list_accounts(site, "").unwrap();
        assert_eq!(accs.len(), 1);
        assert!(accs[0].has_totp && accs[0].has_notes && !accs[0].is_favorite);
        assert_eq!(list_sites().unwrap()[0].n_accounts, 1);

        assert_eq!(
            get_secret(acc, &crypto, "password", true).unwrap(),
            "pw-密码"
        );
        assert_eq!(
            get_secret(acc, &crypto, "totp", false).unwrap(),
            "JBSWY3DPEHPK3PXP"
        );
        // touch 语义：复制后 last_used_at 落库
        assert!(list_accounts(site, "").unwrap()[0].last_used_at.is_some());

        // 搜索：五列 LIKE + 转义
        assert_eq!(search_sites("git").unwrap().len(), 1);
        assert_eq!(search_sites("gh").unwrap().len(), 1); // aliases 命中
        assert_eq!(search_sites("%").unwrap().len(), 0, "通配符按字面匹配");
        assert!(find_by_domain("github.com").unwrap().is_some());
        assert!(find_by_domain("GH").unwrap().is_some()); // 别名分词精确相等
        assert!(find_by_domain("h").unwrap().is_none()); // 子串不得命中别名（防授错域）

        // 更新：None=不改；空密码=不改；空 totp=置 NULL
        update_account(
            acc,
            &crypto,
            &AccountPatch {
                username: Some("alice2".into()),
                password: Some("".into()),
                totp_secret: Some("".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let accs = list_accounts(site, "").unwrap();
        assert_eq!(accs[0].username, "alice2");
        assert!(!accs[0].has_totp, "空 totp 应置 NULL");
        assert_eq!(
            get_secret(acc, &crypto, "password", false).unwrap(),
            "pw-密码",
            "空串密码不改原密文"
        );
        assert!(toggle_favorite(acc).unwrap());
        assert!(list_accounts(site, "").unwrap()[0].is_favorite);

        // global_search：空格分词 AND 直接命中账号级
        assert_eq!(global_search("github alice2").unwrap().len(), 1);
        assert_eq!(global_search("github 不存在").unwrap().len(), 0);

        delete_account(acc).unwrap();
        assert_eq!(list_accounts(site, "").unwrap().len(), 0);
        delete_site(site).unwrap();
        assert!(get_site(site).unwrap().is_none());
    }

    #[test]
    fn init_db_is_idempotent_via_user_version() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        reset_db_ready_for_test();
        init_db().unwrap();
        reset_db_ready_for_test();
        init_db().unwrap(); // 二次调用走 user_version 短路（不重放 DDL）
        let conn = conn().unwrap();
        let ver: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, SCHEMA_VERSION);
    }

    #[test]
    fn bulk_decrypt_downgrades_single_failure() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        reset_db_ready_for_test();
        init_db().unwrap();
        let crypto = crypto_dpapi();
        let site = add_site("S", "", "", "", "").unwrap();
        let a1 = add_account(site, "ok1", &crypto, "", "", "", "").unwrap();
        let a2 = add_account(site, "ok2", &crypto, "", "", "", "").unwrap();
        // 人为弄坏 a1 的密文（模拟换用户/损坏）
        let conn = conn().unwrap();
        conn.execute(
            "UPDATE accounts SET password_enc = x'00FF00' WHERE id=?",
            [a1],
        )
        .unwrap();
        drop(conn);
        let out = get_secrets_bulk(&[a1, a2], &crypto, "password").unwrap();
        assert!(!out.contains_key(&a1), "损坏条目必须缺席，不外溢");
        assert_eq!(out.get(&a2).map(String::as_str), Some("ok2"));
    }
}

#[cfg(test)]
mod cross_language_db_tests {
    use super::*;

    /// fixture 目录（tools/gen_vault_fixture.py 生成；缺失即跳过——CI 无 Python 时照常绿）。
    fn fixture_dir() -> Option<std::path::PathBuf> {
        let dir = std::env::temp_dir().join("sk_vault_xlang");
        (dir.join("vault.db").exists() && dir.join("vault_db_expected.json").exists())
            .then_some(dir)
    }

    #[test]
    fn reads_python_generated_vault_db_end_to_end() {
        let _g = crate::test_support::serial();
        let Some(dir) = fixture_dir() else {
            return; // 无 fixture：跳过（python tools/gen_vault_fixture.py 生成）
        };
        let db_expected: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("vault_db_expected.json")).unwrap(),
        )
        .unwrap();
        let crypto_expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("expected.json")).unwrap())
                .unwrap();
        // DB 路径指 fixture（测试专用覆盖，可重设）；key 走 with_path 显式路径
        set_db_override(dir.clone());
        let mut crypto = VaultCrypto::with_path(dir.join("vault.key"));

        reset_db_ready_for_test();
        init_db().unwrap(); // user_version 已盖章 → 幂等短路，不得改动 Python 库

        let password = crypto_expected["password"].as_str().expect("crypto 密码");
        assert!(
            crypto.unlock(Some(password)) == Ok(true),
            "Python 版加密的 vault.key 必须能被 Rust 版用主密码解锁"
        );

        let site_id = db_expected["site_id"].as_i64().unwrap();
        let site = get_site(site_id).unwrap().expect("site 行");
        assert_eq!(site.name, db_expected["site_name"]);
        let listed = list_sites().unwrap();
        assert_eq!(listed[0].name, db_expected["site_name"]);
        assert_eq!(
            listed[0].n_accounts, 2,
            "账号计数子查询必须读到 Python 数据"
        );

        let acc1 = db_expected["acc1"].as_i64().unwrap();
        let accs = list_accounts(site_id, "").unwrap();
        assert_eq!(accs.len(), 2);
        assert!(accs.iter().find(|a| a.id == acc1).unwrap().has_totp);
        assert!(
            accs.iter()
                .find(|a| a.id == db_expected["acc2"].as_i64().unwrap())
                .unwrap()
                .is_favorite,
            "Python 侧 toggle_favorite 的置顶必须被读到"
        );

        // 解密互通（密码模式 2000 iter 按文件读 + aad=field）
        assert_eq!(
            get_secret(acc1, &crypto, "password", false).unwrap(),
            db_expected["acc1_password"],
        );
        assert_eq!(
            get_secret(acc1, &crypto, "notes", false).unwrap(),
            db_expected["acc1_notes"],
        );

        // TOTP 同密钥同瞬间同码
        let secret = db_expected["acc1_totp_secret"].as_str().unwrap();
        let want = db_expected["expected_totp_at59"].as_str().unwrap();
        assert_eq!(totp_code(secret, Some(59.0), 6, 30), want);

        // 全局搜索（元数据级，不解密）
        assert_eq!(global_search("fixture 主账号").unwrap().len(), 1);

        clear_db_override(); // 恢复默认路径（防污染后续用例）
    }

    #[test]
    fn rust_writes_vault_db_python_can_open() {
        // 反向：Rust 建库写入 → tools/verify_vault_db_rust_to_python.py 校验
        let _g = crate::test_support::serial();
        let out = std::env::temp_dir()
            .join("sk_vault_xlang")
            .join("rust_db_out");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        set_db_override(out.clone());

        reset_db_ready_for_test();
        init_db().unwrap();
        // DPAPI 包裹的数据密钥无法被 Python 校验脚本跨机器读——用密码模式
        // 并把 vault.key 一并导出（verify 脚本用固定密码解锁）
        let key_path = out.join("vault.key");
        let mut crypto = VaultCrypto::with_path(key_path);
        crypto
            .setup(Some("rust-export-pw"))
            .expect("密码模式初始化");

        let site = add_site(
            "RustFixture",
            "rust.example",
            "https://rust.example",
            "别名R",
            "",
        )
        .unwrap();
        add_account(
            site,
            "rust-写入-密码",
            &crypto,
            "Rust 主账号",
            "carol",
            "JBSWY3DPEHPK3PXP",
            "",
        )
        .unwrap();

        // Python 侧校验产物存在性自检（vault.key 与库同目录落盘）
        assert!(out.join("vault.db").exists() && out.join("vault.key").exists());
        clear_db_override();
    }
}
