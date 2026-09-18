"""密码库存储：SQLite sites/accounts 两级模型。

核心原则：网站是容器，账号是条目；搜索只走明文元数据（name/domain/aliases/
username/label/tags），密码/TOTP/备注永远密文落盘、按需解密（get_secret）。
加密由调用方传入的 crypto 对象（vault_crypto.VaultCrypto）承担，本模块不依赖
cryptography，便于离线单测。
"""
import hashlib
import hmac
import os
import re
import sqlite3
import struct
import time

from . import config

SCHEMA = """
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
"""


def _now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S")


def _db_path() -> str:
    # 动态读取（而非 import 时取值）：conftest 会重定向 config.APPDATA_DIR
    return os.path.join(config.APPDATA_DIR, "vault.db")


def _connect() -> sqlite3.Connection:
    path = _db_path()
    os.makedirs(os.path.dirname(path), exist_ok=True)   # 便携首启：data\config 尚不存在
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def init_db():
    with _connect() as conn:
        conn.executescript(SCHEMA)


# ---------------- sites ----------------

def add_site(name: str, domain: str = "", url: str = "",
             aliases: str = "", tags: str = "") -> int:
    with _connect() as conn:
        cur = conn.execute(
            "INSERT INTO sites(name, domain, url, aliases, tags, created_at, updated_at)"
            " VALUES(?,?,?,?,?,?,?)",
            (name, domain, url, aliases, tags, _now(), _now()))
        return int(cur.lastrowid)


def update_site(site_id: int, **fields):
    allowed = {"name", "domain", "url", "aliases", "tags"}
    patch = {k: v for k, v in fields.items() if k in allowed}
    if not patch:
        return
    patch["updated_at"] = _now()
    sets = ",".join(f"{k}=?" for k in patch)
    with _connect() as conn:
        conn.execute(f"UPDATE sites SET {sets} WHERE id=?",
                     (*patch.values(), site_id))


def delete_site(site_id: int):
    with _connect() as conn:
        conn.execute("DELETE FROM accounts WHERE site_id=?", (site_id,))
        conn.execute("DELETE FROM sites WHERE id=?", (site_id,))


def _site_rows(conn, where: str = "", params=()) -> list:
    rows = conn.execute(
        "SELECT s.*, (SELECT COUNT(*) FROM accounts a WHERE a.site_id=s.id)"
        " AS n_accounts FROM sites s " + where + " ORDER BY s.name COLLATE NOCASE",
        params).fetchall()
    return [dict(r) for r in rows]


def list_sites() -> list:
    with _connect() as conn:
        return _site_rows(conn)


def _escape_like(q: str) -> str:
    """LIKE 通配符转义（配 ESCAPE '\' 使用）：用户输入的 % / _ 按字面匹配。"""
    return q.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")


def search_sites(q: str) -> list:
    """匹配 name/domain/url/aliases/tags；显示账号数量。"""
    q = _escape_like((q or "").strip().lower())
    with _connect() as conn:
        if not q:
            return _site_rows(conn)
        like = f"%{q}%"
        cond = ("WHERE lower(name) LIKE ? ESCAPE '\\' OR lower(coalesce(domain,'')) LIKE ? ESCAPE '\\'"
                " OR lower(coalesce(url,'')) LIKE ? ESCAPE '\\'"
                " OR lower(coalesce(aliases,'')) LIKE ? ESCAPE '\\'"
                " OR lower(coalesce(tags,'')) LIKE ? ESCAPE '\\'")
        return _site_rows(conn, cond, (like,) * 5)


def find_by_domain(domain: str) -> dict | None:
    """按域名精确匹配（为浏览器扩展/自动填充预留的 API 边界）。

    别名走「逗号分词后精确相等」而非子串 LIKE：子串会让 mail.com 命中
    别名 gmail.com 的站点——自动填充接入后即凭据授错域。
    """
    domain = (domain or "").lower().strip()
    if not domain:
        return None
    with _connect() as conn:
        rows = conn.execute(
            "SELECT * FROM sites WHERE lower(domain)=? OR lower(url)=?",
            (domain, f"https://{domain}")).fetchall()
        if not rows:
            candidates = conn.execute(
                "SELECT * FROM sites WHERE lower(coalesce(aliases,'')) <> ''").fetchall()
            rows = []
            for r in candidates:
                toks = [t.strip().lower()
                        for t in re.split(r"[,，;；]", r["aliases"] or "") if t.strip()]
                if domain in toks:
                    rows.append(r)
        return dict(rows[0]) if rows else None


def get_site(site_id: int) -> dict | None:
    with _connect() as conn:
        r = conn.execute("SELECT * FROM sites WHERE id=?", (site_id,)).fetchone()
        return dict(r) if r else None


# ---------------- accounts ----------------

def add_account(site_id: int, password: str, crypto, label: str = "",
                username: str = "", totp_secret: str = "", notes: str = "") -> int:
    """新增账号。密码必须非空；secrets 由 crypto 加密（添加操作本身不做密码验证）。"""
    if not password:
        raise ValueError("密码不能为空")
    now = _now()
    with _connect() as conn:
        cur = conn.execute(
            "INSERT INTO accounts(site_id, label, username, password_enc, totp_enc,"
            " notes_enc, created_at, updated_at) VALUES(?,?,?,?,?,?,?,?)",
            (site_id, label, username, crypto.encrypt(password.encode("utf-8")),
             crypto.encrypt(totp_secret.encode("utf-8")) if totp_secret else None,
             crypto.encrypt(notes.encode("utf-8")) if notes else None,
             now, now))
        return int(cur.lastrowid)


def update_account(account_id: int, crypto, label=None, username=None,
                   password=None, totp_secret=None, notes=None):
    patch = {}
    if label is not None:
        patch["label"] = label
    if username is not None:
        patch["username"] = username
    if password:
        patch["password_enc"] = crypto.encrypt(password.encode("utf-8"))
    if totp_secret is not None:
        patch["totp_enc"] = (crypto.encrypt(totp_secret.encode("utf-8"))
                             if totp_secret else None)
    if notes is not None:
        patch["notes_enc"] = (crypto.encrypt(notes.encode("utf-8"))
                              if notes else None)
    if not patch:
        return
    patch["updated_at"] = _now()
    sets = ",".join(f"{k}=?" for k in patch)
    with _connect() as conn:
        conn.execute(f"UPDATE accounts SET {sets} WHERE id=?",
                     (*patch.values(), account_id))


def delete_account(account_id: int):
    with _connect() as conn:
        conn.execute("DELETE FROM accounts WHERE id=?", (account_id,))


def toggle_favorite(account_id: int) -> bool:
    with _connect() as conn:
        cur = conn.execute(
            "UPDATE accounts SET is_favorite = 1 - is_favorite WHERE id=?",
            (account_id,))
        return cur.rowcount > 0


def mark_used(account_id: int):
    with _connect() as conn:
        conn.execute("UPDATE accounts SET last_used_at=? WHERE id=?",
                     (_now(), account_id))


def list_accounts(site_id: int, q: str = "") -> list:
    """账号元数据列表（不含任何明文 secrets）。q 匹配 username/label。"""
    q = _escape_like((q or "").strip().lower())
    with _connect() as conn:
        cond, params = "WHERE a.site_id=?", [site_id]
        if q:
            cond += (" AND (lower(coalesce(a.username,'')) LIKE ? ESCAPE '\\'"
                     " OR lower(coalesce(a.label,'')) LIKE ? ESCAPE '\\')")
            params += [f"%{q}%", f"%{q}%"]
        rows = conn.execute(
            "SELECT a.id, a.site_id, a.label, a.username, a.is_favorite,"
            " a.last_used_at, a.totp_enc IS NOT NULL AS has_totp,"
            " a.notes_enc IS NOT NULL AS has_notes"
            f" FROM accounts a {cond}"
            " ORDER BY a.is_favorite DESC, a.username COLLATE NOCASE",
            params).fetchall()
        return [dict(r) for r in rows]


def get_secret(account_id: int, crypto, field: str = "password",
               touch: bool = True) -> str:
    """按需解密：field ∈ password|totp|notes。touch=True 更新 last_used_at
    （复制语义）；整列明文显示等批量读取应传 False，避免 N 次写库。"""
    if field not in ("password", "totp", "notes"):
        raise ValueError(f"未知字段 {field}")
    col = {"password": "password_enc", "totp": "totp_enc", "notes": "notes_enc"}[field]
    with _connect() as conn:
        row = conn.execute(f"SELECT {col} FROM accounts WHERE id=?",
                           (account_id,)).fetchone()
    if not row or row[col] is None:
        return ""
    if touch:
        mark_used(account_id)
    return crypto.decrypt(row[col]).decode("utf-8", "replace")


def _account_row(conn, account_id: int):
    r = conn.execute(
        "SELECT a.*, s.name AS site_name, s.domain AS site_domain FROM accounts a"
        " JOIN sites s ON s.id=a.site_id WHERE a.id=?", (account_id,)).fetchone()
    return dict(r) if r else None


def global_search(q: str) -> list:
    """全局搜索：空格分词 AND，同时匹配网站名/域名/别名/标签与用户名/label，
    直接命中账号级（如 "github alice"）。只返回元数据，不解密。"""
    tokens = [t.lower() for t in (q or "").split() if t.strip()]
    if not tokens:
        return []
    with _connect() as conn:
        rows = conn.execute(
            "SELECT a.id, a.site_id, a.label, a.username, a.is_favorite,"
            " a.totp_enc IS NOT NULL AS has_totp,"
            " s.name AS site_name, s.domain AS site_domain,"
            " coalesce(s.aliases,'') AS site_aliases, coalesce(s.tags,'') AS site_tags"
            " FROM accounts a JOIN sites s ON s.id=a.site_id").fetchall()
    out = []
    for r in rows:
        d = dict(r)
        hay = " ".join([
            d["site_name"], d["site_domain"], d["site_aliases"], d["site_tags"],
            d["username"] or "", d["label"] or "",
        ]).lower()
        if all(t in hay for t in tokens):
            out.append({k: v for k, v in d.items()
                        if k not in ("password_enc", "totp_enc", "notes_enc")})
    return out


# ---------------- TOTP（RFC 6238，stdlib 实现） ----------------

def totp_code(secret_b32: str, at: float = None, digits: int = 6, period: int = 30) -> str:
    """Base32 密钥 → TOTP 验证码。空密钥或密钥非法返回 ""（UI 提示密钥无效）。

    RFC 4648 base32 字母表只有 A-Z2-7：兼容「已带 = padding」「含空格/连字符」
    的常见粘贴格式；含 0/1/8/9 等字母表外字符属录入错误，返回空串而非抛异常
    （此前 binascii.Error 会从 Qt 槽里穿透，TOTP 功能整体静默失效）。
    """
    import base64
    clean = re.sub(r"[\s\-=]", "", (secret_b32 or "").upper())
    if not clean:
        return ""
    if any(c not in "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567" for c in clean):
        return ""
    try:
        key = base64.b32decode(clean + "=" * (-len(clean) % 8))
    except Exception:
        return ""
    counter = int((at if at is not None else time.time()) // period)
    digest = hmac.new(key, struct.pack(">Q", counter), hashlib.sha1).digest()
    offset = digest[-1] & 0x0F
    code = (struct.unpack(">I", digest[offset:offset + 4])[0] & 0x7FFFFFFF) % (10 ** digits)
    return str(code).zfill(digits)
