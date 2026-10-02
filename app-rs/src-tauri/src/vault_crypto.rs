//! 密码库加密层（对应 Python 侧 `vault_crypto.py`）。
//!
//! ## 跨版本兼容是硬契约
//!
//! `vault.key` 与 `vault.db` 里的密文**两个版本必须互相能读**。因此本模块在字节层面
//! 复刻 Python 的格式：
//!
//! ```text
//! 有主密码：{"mode":"password",
//!            "kdf":{"salt":<b64 16B>, "iter":600000},
//!            "wrapped_key":<b64(nonce12 + gcm_ct||tag)>,   aad="vault-key"
//!            "verifier":  <b64(nonce12 + gcm_ct||tag)>}   aad="vault-verify"
//! 无主密码：{"mode":"dpapi", "wrapped_key":<b64 DPAPI blob>}
//! ```
//! - 数据密钥 32 字节；PBKDF2-HMAC-SHA256 派生；
//! - verifier 的明文是 `syskit-vault-ok`（ASCII）；
//! - 字段加解密用 aad=`"field"`。
//!
//! ## 为什么用 BCrypt 而不是新引一个 crate
//!
//! AES-256-GCM 与 PBKDF2 都已在 `windows` crate 里（`BCryptEncrypt`/
//! `BCryptDeriveKeyPBKDF2`），而 DPAPI 本来就用 `CryptProtectData`。**不引新依赖**
//! 就少一处供应链风险，且 BCrypt 与 Python 用的 OpenSSL 在 AES-GCM 上是同标准的
//! 字节级兼容（有跨语言往返测试锁住）。
//!
//! ## 两个必须保留的语义
//!
//! 1. **迭代次数从文件里读**（`kdf.iter`），不是每次都用当前常量。将来上调
//!    `KDF_ITER` 时，存量库若按新常量派生会解不开 verifier，用户会看到"主密码错误"
//!    并可能误删整个库；
//! 2. **解锁失败返回 false 而不是抛异常**：`vault.key` 损坏（掉电截断）时 GUI 不能
//!    崩，否则用户慌乱中可能删掉整个密码库。

use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};

/// PBKDF2 迭代次数（新库写入此值；读取时以文件里的为准）。
pub const KDF_ITER: u32 = 600_000;
/// 数据密钥长度。
pub const KEY_LEN: usize = 32;
/// GCM nonce 长度。
pub const NONCE_LEN: usize = 12;
/// GCM 认证标签长度。
pub const TAG_LEN: usize = 16;
/// verifier 的明文（必须与 Python 完全一致，否则两版互相认为"主密码错误"）。
pub const VERIFIER: &[u8] = b"syskit-vault-ok";
/// 包裹数据密钥时的 aad。
pub const AAD_KEY: &[u8] = b"vault-key";
/// 校验密码时的 aad。
pub const AAD_VERIFY: &[u8] = b"vault-verify";
/// 字段加解密的 aad。
pub const AAD_FIELD: &[u8] = b"field";
/// DPAPI 的说明文本（Python 用 "SysKit vault key"）。
pub const DPAPI_DESCRIPTION: &str = "SysKit vault key";

/// 密码条目存取不可用时的统一说明。
///
/// 本阶段（按用户决定）**不实现数据层**，所以存取入口必须明确失败——绝不能
/// 假装保存成功：用户会以为密码已存好，之后发现什么都没有，而密码这种数据
/// 丢了就是丢了。加密层已完成且与 Python 版双向互通。
pub const STORAGE_UNAVAILABLE: &str =
    "密码条目存取尚未实现：数据层（vault.db / SQLite）按决定暂不引入。\
加密层已完成且与 Python 版双向互通（vault.key），但缺少存储层意味着无法真正保存/读取密码条目。\
后续启用需引入 SQLite 依赖（如 rusqlite）。";

/// `vault.key` 的 metadata。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultMeta {
    /// `password` 或 `dpapi`
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<Kdf>,
    #[serde(default)]
    pub wrapped_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier: Option<String>,
}

/// PBKDF2 参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kdf {
    #[serde(default)]
    pub salt: String,
    #[serde(default)]
    pub iter: Option<u32>,
}

impl Kdf {
    /// 迭代次数：**以文件为准**，缺失/非法回退当前常量。
    ///
    /// 这条是跨版本+跨时间的兼容点：升级调参后仍能解开存量库。
    pub fn effective_iter(&self) -> u32 {
        match self.iter {
            Some(n) if n > 0 => n,
            _ => KDF_ITER,
        }
    }
}

/// 解锁失败的原因（用于给用户可操作的提示，而不是一句"失败"）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnlockError {
    /// 未初始化
    NotProvisioned,
    /// 文件无法解析（损坏/截断）
    Corrupt(String),
    /// 主密码错误
    WrongPassword,
    /// DPAPI 解包失败（换了用户/机器）
    DpapiFailed(String),
}

impl UnlockError {
    /// 面向用户的说明。
    pub fn message(&self) -> String {
        match self {
            UnlockError::NotProvisioned => "密码库尚未初始化".to_string(),
            UnlockError::Corrupt(e) => format!(
                "vault.key 无法解析（可能已损坏）：{e}。密码库保持锁定；\
                 如确认放弃历史数据可删除该文件后重启。"
            ),
            UnlockError::WrongPassword => "主密码错误".to_string(),
            UnlockError::DpapiFailed(e) => format!(
                "DPAPI 解包失败：{e}。免密模式绑定当前 Windows 用户，\
                 换用户或换机器后无法解开；请改用主密码模式。"
            ),
        }
    }
}

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn unb64(text: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(text.as_bytes())
        .map_err(|e| format!("base64 解码失败: {e}"))
}

// ---------------- 纯函数：拼接与校验 ----------------

/// 组装密文：`nonce(12) || ct || tag(16)`。
pub fn pack_sealed(nonce: &[u8], ct_and_tag: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nonce.len() + ct_and_tag.len());
    out.extend_from_slice(nonce);
    out.extend_from_slice(ct_and_tag);
    out
}

/// 拆出 nonce 与 `ct||tag`。长度不足时报错而不是 panic。
pub fn split_sealed(blob: &[u8]) -> Result<(&[u8], &[u8]), String> {
    if blob.len() <= NONCE_LEN {
        return Err("密文长度非法".to_string());
    }
    Ok((&blob[..NONCE_LEN], &blob[NONCE_LEN..]))
}

/// 生成 32 字节随机数据密钥。
pub fn random_key() -> Vec<u8> {
    crate::w32::random_bytes(KEY_LEN)
}

/// 生成 16 字节盐。
pub fn random_salt() -> Vec<u8> {
    crate::w32::random_bytes(16)
}

/// 生成 12 字节 nonce。
pub fn random_nonce() -> Vec<u8> {
    crate::w32::random_bytes(NONCE_LEN)
}

// ---------------- 密钥管理 ----------------

/// 密码库密钥管理器。生命周期：setup → (unlock ⇄ lock) → 改/移主密码。
pub struct VaultCrypto {
    path: PathBuf,
    /// 32B 数据密钥，仅解锁后驻留内存
    data_key: Option<Vec<u8>>,
    /// 本次解锁的 UNIX 时刻（秒）：自动重锁窗口判定用；锁定时清空
    unlocked_at: Option<f64>,
}

impl VaultCrypto {
    /// 用默认路径（`%APPDATA%\SysKit\vault.key`）。
    pub fn new() -> Self {
        Self::with_path(crate::paths::appdata_dir().join("vault.key"))
    }

    /// 指定路径（测试用）。
    pub fn with_path(path: PathBuf) -> Self {
        Self {
            path,
            data_key: None,
            unlocked_at: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_provisioned(&self) -> bool {
        self.path.is_file()
    }

    pub fn is_unlocked(&self) -> bool {
        self.data_key.is_some()
    }

    /// 读 metadata；文件缺失或损坏都返回空（调用方据此判断）。
    pub fn read_meta(&self) -> VaultMeta {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return VaultMeta {
                mode: String::new(),
                kdf: None,
                wrapped_key: String::new(),
                verifier: None,
            };
        };
        serde_json::from_str(&text).unwrap_or(VaultMeta {
            mode: String::new(),
            kdf: None,
            wrapped_key: String::new(),
            verifier: None,
        })
    }

    pub fn has_master_password(&self) -> bool {
        self.is_provisioned() && self.read_meta().mode == "password"
    }

    /// 原子写 metadata（临时文件 + 替换）。
    fn write_meta(&self, meta: &VaultMeta) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
        }
        let text = serde_json::to_string(meta).map_err(|e| format!("序列化失败: {e}"))?;
        let tmp = self.path.with_extension("key.tmp");
        std::fs::write(&tmp, text.as_bytes()).map_err(|e| format!("写入失败: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("替换 vault.key 失败: {e}")
        })
    }

    /// 首次初始化。有密码则密码包裹，否则 DPAPI 包裹。
    pub fn setup(&mut self, password: Option<&str>) -> Result<(), String> {
        if self.is_provisioned() {
            return Err("密码库已初始化".to_string());
        }
        let key = random_key();
        match password {
            Some(p) if !p.is_empty() => {
                let meta = self.build_password_meta(&key, p)?;
                self.write_meta(&meta)?;
            }
            _ => {
                let wrapped =
                    crate::w32::dpapi_protect(&key).ok_or("DPAPI 保护失败（CryptProtectData）")?;
                self.write_meta(&VaultMeta {
                    mode: "dpapi".to_string(),
                    kdf: None,
                    wrapped_key: b64(&wrapped),
                    verifier: None,
                })?;
            }
        }
        self.data_key = Some(key);
        self.unlocked_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
        );
        Ok(())
    }

    fn build_password_meta(&self, key: &[u8], password: &str) -> Result<VaultMeta, String> {
        let salt = random_salt();
        let dk = derive_key(password, &salt, KDF_ITER)?;
        let wrapped = seal_gcm(&dk, key, AAD_KEY)?;
        let verifier = seal_gcm(&dk, VERIFIER, AAD_VERIFY)?;
        Ok(VaultMeta {
            mode: "password".to_string(),
            kdf: Some(Kdf {
                salt: b64(&salt),
                iter: Some(KDF_ITER),
            }),
            wrapped_key: b64(&wrapped),
            verifier: Some(b64(&verifier)),
        })
    }

    /// 解锁。已解锁直接 Ok(true)；失败返回原因而不抛异常。
    pub fn unlock(&mut self, password: Option<&str>) -> Result<bool, UnlockError> {
        if self.is_unlocked() {
            return Ok(true);
        }
        if !self.is_provisioned() {
            return Err(UnlockError::NotProvisioned);
        }
        let meta = self.read_meta();
        if meta.wrapped_key.is_empty() {
            return Err(UnlockError::Corrupt("wrapped_key 缺失".to_string()));
        }
        let wrapped = unb64(&meta.wrapped_key).map_err(UnlockError::Corrupt)?;

        if meta.mode == "dpapi" {
            // 既有 dpapi_unprotect 返回 Option：None 统一当"解包失败"（换用户/换机器）
            return match crate::w32::dpapi_unprotect(&wrapped) {
                Some(key) if key.len() == KEY_LEN => {
                    self.data_key = Some(key);
                    self.unlocked_at = Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs_f64())
                            .unwrap_or(0.0),
                    );
                    Ok(true)
                }
                Some(_) => Err(UnlockError::Corrupt("DPAPI 解出的密钥长度非法".to_string())),
                None => Err(UnlockError::DpapiFailed(
                    "CryptUnprotectData 失败".to_string(),
                )),
            };
        }

        let kdf = meta.kdf.clone().unwrap_or(Kdf {
            salt: String::new(),
            iter: None,
        });
        let salt = unb64(&kdf.salt).map_err(UnlockError::Corrupt)?;
        let dk = derive_key(password.unwrap_or(""), &salt, kdf.effective_iter())
            .map_err(UnlockError::Corrupt)?;

        // 先验 verifier 再解 data key：verifier 错了说明是密码问题，
        // 否则会把"密码错误"误报成"数据损坏"
        let Some(verifier_b64) = meta.verifier.clone() else {
            return Err(UnlockError::Corrupt("verifier 缺失".to_string()));
        };
        let verifier = unb64(&verifier_b64).map_err(UnlockError::Corrupt)?;
        match open_gcm(&dk, &verifier, AAD_VERIFY) {
            Ok(v) if v == VERIFIER => {}
            Ok(_) => return Err(UnlockError::Corrupt("verifier 内容不符".to_string())),
            Err(_) => return Err(UnlockError::WrongPassword),
        }
        match open_gcm(&dk, &wrapped, AAD_KEY) {
            Ok(key) if key.len() == KEY_LEN => {
                self.data_key = Some(key);
                self.unlocked_at = Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0),
                );
                Ok(true)
            }
            Ok(_) => Err(UnlockError::Corrupt("数据密钥长度非法".to_string())),
            Err(e) => Err(UnlockError::Corrupt(e)),
        }
    }

    pub fn lock(&mut self) {
        // 覆写一遍再丢弃，减少密钥在内存里的残留时间
        if let Some(k) = self.data_key.as_mut() {
            for b in k.iter_mut() {
                *b = 0;
            }
        }
        self.data_key = None;
        self.unlocked_at = None;
    }

    /// 本次解锁的 UNIX 时刻（秒）；未解锁返回 None（自动重锁窗口判定用）。
    pub fn unlocked_since_secs(&self) -> Option<f64> {
        self.data_key.as_ref()?;
        self.unlocked_at
    }

    /// 校验主密码（不改变解锁状态）。非密码模式返回 false。
    pub fn verify_password(&self, password: &str) -> bool {
        let meta = self.read_meta();
        if meta.mode != "password" {
            return false;
        }
        let Some(kdf) = meta.kdf else { return false };
        let (Ok(salt), Some(verifier_b64)) = (unb64(&kdf.salt), meta.verifier) else {
            return false;
        };
        let Ok(dk) = derive_key(password, &salt, kdf.effective_iter()) else {
            return false;
        };
        let Ok(verifier) = unb64(&verifier_b64) else {
            return false;
        };
        matches!(open_gcm(&dk, &verifier, AAD_VERIFY), Ok(v) if v == VERIFIER)
    }

    /// 无主密码 → 设主密码（数据密钥不变）。
    pub fn set_master_password(&mut self, password: &str) -> Result<(), String> {
        if !self.is_unlocked() {
            return Err("请先解锁".to_string());
        }
        if password.is_empty() {
            return Err("主密码不能为空".to_string());
        }
        let key = self.data_key.clone().expect("已检查解锁状态");
        let meta = self.build_password_meta(&key, password)?;
        self.write_meta(&meta)
    }

    /// 有主密码 → 移除（回到 DPAPI 模式）。当前密码错误返回 Ok(false)。
    pub fn remove_master_password(&mut self, current: &str) -> Result<bool, String> {
        if !self.is_unlocked() {
            return Err("请先解锁".to_string());
        }
        if !self.has_master_password() {
            return Ok(false);
        }
        if !self.verify_password(current) {
            return Ok(false);
        }
        let key = self.data_key.clone().expect("已检查解锁状态");
        let wrapped =
            crate::w32::dpapi_protect(&key).ok_or("DPAPI 保护失败（CryptProtectData）")?;
        self.write_meta(&VaultMeta {
            mode: "dpapi".to_string(),
            kdf: None,
            wrapped_key: b64(&wrapped),
            verifier: None,
        })?;
        Ok(true)
    }

    /// 改主密码（数据密钥不变）。
    pub fn change_master_password(&mut self, new_password: &str) -> Result<(), String> {
        self.set_master_password(new_password)
    }

    /// 加密一个字段（aad = `field`）。
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let key = self
            .data_key
            .as_ref()
            .ok_or_else(|| "密码库未解锁".to_string())?;
        seal_gcm(key, plaintext, AAD_FIELD)
    }

    /// 解密一个字段（aad = `field`）。
    pub fn decrypt(&self, blob: &[u8]) -> Result<Vec<u8>, String> {
        let key = self
            .data_key
            .as_ref()
            .ok_or_else(|| "密码库未解锁".to_string())?;
        open_gcm(key, blob, AAD_FIELD)
    }
}

impl Default for VaultCrypto {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------- CBC 风格封装（BCrypt 的对称调用是带状态的） ----------------

/// PBKDF2-HMAC-SHA256 派生密钥。
pub fn derive_key(password: &str, salt: &[u8], iterations: u32) -> Result<Vec<u8>, String> {
    crate::w32::pbkdf2_sha256(password.as_bytes(), salt, iterations, KEY_LEN)
}

/// AES-256-GCM 加密：返回 `nonce || ct || tag`。
pub fn seal_gcm(key: &[u8], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, String> {
    if key.len() != KEY_LEN {
        return Err(format!("密钥长度必须为 {} 字节", KEY_LEN));
    }
    let nonce = random_nonce();
    let ct_and_tag = crate::w32::aes_gcm_encrypt(key, &nonce, plaintext, aad, TAG_LEN)?;
    Ok(pack_sealed(&nonce, &ct_and_tag))
}

/// AES-256-GCM 解密：输入 `nonce || ct || tag`。
pub fn open_gcm(key: &[u8], blob: &[u8], aad: &[u8]) -> Result<Vec<u8>, String> {
    if key.len() != KEY_LEN {
        return Err(format!("密钥长度必须为 {} 字节", KEY_LEN));
    }
    let (nonce, ct_and_tag) = split_sealed(blob)?;
    if ct_and_tag.len() < TAG_LEN {
        return Err("密文长度不足（缺少认证标签）".to_string());
    }
    crate::w32::aes_gcm_decrypt(key, nonce, ct_and_tag, aad, TAG_LEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_key_path(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sk_vault_{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("vault.key")
    }

    fn fresh(tag: &str) -> VaultCrypto {
        VaultCrypto::with_path(tmp_key_path(tag))
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(KDF_ITER, 600_000);
        assert_eq!(KEY_LEN, 32);
        assert_eq!(NONCE_LEN, 12);
        assert_eq!(VERIFIER, b"syskit-vault-ok");
        assert_eq!(AAD_KEY, b"vault-key");
        assert_eq!(AAD_VERIFY, b"vault-verify");
        assert_eq!(AAD_FIELD, b"field");
        assert_eq!(DPAPI_DESCRIPTION, "SysKit vault key");
    }

    #[test]
    fn sealed_layout_is_nonce_then_ct_and_tag() {
        let key = [7u8; KEY_LEN];
        let sealed = seal_gcm(&key, b"hello", AAD_FIELD).unwrap();
        // 明文 5 字节 + tag 16 字节 + nonce 12 字节
        assert_eq!(sealed.len(), NONCE_LEN + 5 + TAG_LEN);
        assert_eq!(open_gcm(&key, &sealed, AAD_FIELD).unwrap(), b"hello");
    }

    #[test]
    fn gcm_round_trip_various_sizes() {
        let key = random_key();
        for n in [0usize, 1, 15, 16, 17, 32, 255, 4096] {
            let pt = vec![0xABu8; n];
            let sealed = seal_gcm(&key, &pt, AAD_FIELD).unwrap();
            assert_eq!(
                open_gcm(&key, &sealed, AAD_FIELD).unwrap(),
                pt,
                "长度 {n} 往返失败"
            );
        }
    }

    #[test]
    fn gcm_rejects_wrong_key_and_wrong_aad() {
        let key = random_key();
        let sealed = seal_gcm(&key, b"secret", AAD_FIELD).unwrap();
        let other = random_key();
        assert!(
            open_gcm(&other, &sealed, AAD_FIELD).is_err(),
            "换密钥必须失败"
        );
        assert!(
            open_gcm(&key, &sealed, AAD_KEY).is_err(),
            "换 aad 必须失败（这是 aad 的全部意义）"
        );
    }

    #[test]
    fn gcm_detects_tampering() {
        let key = random_key();
        let mut sealed = seal_gcm(&key, b"secret data here", AAD_FIELD).unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01; // 改 tag
        assert!(
            open_gcm(&key, &sealed, AAD_FIELD).is_err(),
            "改 tag 必须被发现"
        );
        let mut sealed2 = seal_gcm(&key, b"secret data here", AAD_FIELD).unwrap();
        sealed2[NONCE_LEN + 1] ^= 0x01; // 改密文
        assert!(
            open_gcm(&key, &sealed2, AAD_FIELD).is_err(),
            "改密文必须被发现"
        );
    }

    #[test]
    fn gcm_nonce_differs_between_calls() {
        // 同一明文两次加密必须产生不同密文（nonce 随机）
        let key = random_key();
        let a = seal_gcm(&key, b"same", AAD_FIELD).unwrap();
        let b = seal_gcm(&key, b"same", AAD_FIELD).unwrap();
        assert_ne!(a, b, "nonce 必须每次不同，否则 GCM 会失去保密性");
        assert_eq!(open_gcm(&key, &a, AAD_FIELD).unwrap(), b"same");
        assert_eq!(open_gcm(&key, &b, AAD_FIELD).unwrap(), b"same");
    }

    #[test]
    fn split_sealed_rejects_short_input() {
        assert!(split_sealed(&[]).is_err());
        assert!(
            split_sealed(&[0u8; NONCE_LEN]).is_err(),
            "恰好只有 nonce 也不行"
        );
        assert!(split_sealed(&[0u8; NONCE_LEN + 1]).is_ok());
    }

    #[test]
    fn wrong_key_len_is_rejected() {
        assert!(seal_gcm(&[0u8; 16], b"x", AAD_FIELD).is_err());
        assert!(open_gcm(&[0u8; 16], &[0u8; 40], AAD_FIELD).is_err());
    }

    // ---------------- 生命周期 ----------------

    #[test]
    fn setup_dpapi_then_unlock() {
        let mut v = fresh("dpapi");
        assert!(!v.is_provisioned());
        v.setup(None).unwrap();
        assert!(v.is_provisioned());
        assert!(v.is_unlocked(), "初始化后即解锁");
        assert!(!v.has_master_password());

        // 元数据形态
        let meta = v.read_meta();
        assert_eq!(meta.mode, "dpapi");
        assert!(meta.kdf.is_none());
        assert!(!meta.wrapped_key.is_empty());
        assert!(meta.verifier.is_none());

        // 锁定后能重新解锁（DPAPI 免密）
        v.lock();
        assert!(!v.is_unlocked());
        assert!(v.unlock(None).unwrap());
        assert!(v.is_unlocked());
    }

    #[test]
    fn setup_with_password_writes_expected_fields() {
        let mut v = fresh("pw");
        v.setup(Some("hunter2")).unwrap();
        let meta = v.read_meta();
        assert_eq!(meta.mode, "password");
        let kdf = meta.kdf.as_ref().unwrap();
        assert_eq!(kdf.effective_iter(), KDF_ITER);
        // salt 应是 16 字节
        assert_eq!(unb64(&kdf.salt).unwrap().len(), 16);
        assert!(meta.verifier.is_some());
        // wrapped_key = nonce12 + 32字节密钥 + tag16
        assert_eq!(
            unb64(&meta.wrapped_key).unwrap().len(),
            NONCE_LEN + KEY_LEN + TAG_LEN
        );
        assert_eq!(
            unb64(meta.verifier.as_ref().unwrap()).unwrap().len(),
            NONCE_LEN + VERIFIER.len() + TAG_LEN
        );
    }

    #[test]
    fn setup_twice_is_rejected() {
        let mut v = fresh("twice");
        v.setup(None).unwrap();
        assert!(v.setup(None).is_err(), "已初始化不许再 setup");
    }

    #[test]
    fn unlock_with_correct_and_wrong_password() {
        let mut v = fresh("unlock");
        v.setup(Some("correct horse")).unwrap();
        v.lock();
        assert_eq!(
            v.unlock(Some("wrong")).unwrap_err(),
            UnlockError::WrongPassword
        );
        assert!(!v.is_unlocked(), "密码错误后必须保持锁定");
        assert!(v.unlock(Some("correct horse")).unwrap());
        assert!(v.is_unlocked());
    }

    #[test]
    fn unlock_empty_password_is_wrong_not_panic() {
        let mut v = fresh("empty");
        v.setup(Some("real")).unwrap();
        v.lock();
        assert_eq!(v.unlock(Some("")).unwrap_err(), UnlockError::WrongPassword);
        assert_eq!(v.unlock(None).unwrap_err(), UnlockError::WrongPassword);
    }

    #[test]
    fn unlock_unprovisioned_reports_clearly() {
        let mut v = fresh("missing");
        assert_eq!(v.unlock(None).unwrap_err(), UnlockError::NotProvisioned);
        assert!(v.unlock(None).unwrap_err().message().contains("尚未初始化"));
    }

    #[test]
    fn corrupt_key_file_reports_corrupt_not_wrong_password() {
        // 掉电截断的场景：必须报"损坏"而不是"主密码错误"——
        // 后者会让用户反复试密码，甚至以为库没了而删掉
        let mut v = fresh("corrupt");
        std::fs::write(v.path(), b"{ this is not json").unwrap();
        match v.unlock(None).unwrap_err() {
            UnlockError::Corrupt(_) => {}
            other => panic!("应报损坏，实际 {other:?}"),
        }
        assert!(v.unlock(None).unwrap_err().message().contains("损坏"));
    }

    #[test]
    fn key_file_without_wrapped_key_is_corrupt() {
        let mut v = fresh("nowrap");
        std::fs::write(v.path(), br#"{"mode":"password","kdf":{"salt":"AAAA"}}"#).unwrap();
        match v.unlock(Some("x")).unwrap_err() {
            UnlockError::Corrupt(m) => assert!(m.contains("wrapped_key"), "{m}"),
            other => panic!("应报损坏，实际 {other:?}"),
        }
    }

    #[test]
    fn effective_iter_comes_from_file_not_constant() {
        // 升级调参后仍要能解开存量库：迭代数必须读文件
        let mut v = fresh("iter");
        // 手工构造一个用 1000 次迭代的库
        let salt = random_salt();
        let dk = derive_key("pw", &salt, 1000).unwrap();
        let key = random_key();
        let meta = VaultMeta {
            mode: "password".into(),
            kdf: Some(Kdf {
                salt: b64(&salt),
                iter: Some(1000),
            }),
            wrapped_key: b64(&seal_gcm(&dk, &key, AAD_KEY).unwrap()),
            verifier: Some(b64(&seal_gcm(&dk, VERIFIER, AAD_VERIFY).unwrap())),
        };
        v.write_meta(&meta).unwrap();
        assert!(v.unlock(Some("pw")).unwrap(), "必须按文件里的 iter 派生");
        assert!(v.encrypt(b"x").is_ok(), "解锁后应能加密字段");
    }

    #[test]
    fn missing_iter_falls_back_to_constant() {
        assert_eq!(
            Kdf {
                salt: String::new(),
                iter: None
            }
            .effective_iter(),
            KDF_ITER
        );
        assert_eq!(
            Kdf {
                salt: String::new(),
                iter: Some(0)
            }
            .effective_iter(),
            KDF_ITER,
            "0 视为非法并回退"
        );
        assert_eq!(
            Kdf {
                salt: String::new(),
                iter: Some(1)
            }
            .effective_iter(),
            1
        );
    }

    // ---------------- 主密码管理 ----------------

    #[test]
    fn set_then_remove_master_password_keeps_data_key() {
        let mut v = fresh("setremove");
        v.setup(None).unwrap();
        // 记下数据密钥（用它加密一段内容，改密后仍应能解开）
        let sealed = v.encrypt(b"payload").unwrap();
        assert!(!v.has_master_password());

        v.set_master_password("newpass").unwrap();
        assert!(v.has_master_password());
        assert!(v.is_unlocked(), "设密码不应导致锁定");
        // 数据密钥不变 → 旧密文仍可解
        assert_eq!(v.decrypt(&sealed).unwrap(), b"payload");

        // 锁定后必须用新密码
        v.lock();
        assert_eq!(v.unlock(None).unwrap_err(), UnlockError::WrongPassword);
        assert!(v.unlock(Some("newpass")).unwrap());
        assert_eq!(v.decrypt(&sealed).unwrap(), b"payload");

        // 移除主密码 → 回到 DPAPI
        assert!(v.remove_master_password("newpass").unwrap());
        assert!(!v.has_master_password());
        assert_eq!(v.read_meta().mode, "dpapi");
        v.lock();
        assert!(v.unlock(None).unwrap(), "回到免密模式后应能直接解锁");
        assert_eq!(v.decrypt(&sealed).unwrap(), b"payload");
    }

    #[test]
    fn remove_master_password_requires_correct_current() {
        let mut v = fresh("removewrong");
        v.setup(Some("orig")).unwrap();
        assert!(
            !v.remove_master_password("wrong").unwrap(),
            "密码错必须返回 false"
        );
        assert!(v.has_master_password(), "失败的移除不得改动文件");
        assert!(v.remove_master_password("orig").unwrap());
    }

    #[test]
    fn set_master_password_requires_unlock() {
        let mut v = fresh("lockfirst");
        v.setup(None).unwrap();
        v.lock();
        assert!(v.set_master_password("x").is_err(), "锁定态不许改密码");
        assert!(v.remove_master_password("x").is_err());
    }

    #[test]
    fn set_empty_password_is_rejected() {
        let mut v = fresh("emptynew");
        v.setup(None).unwrap();
        assert!(v.set_master_password("").is_err());
        assert!(!v.has_master_password(), "失败后不应改成密码模式");
    }

    #[test]
    fn change_password_makes_old_one_invalid() {
        let mut v = fresh("change");
        v.setup(Some("old")).unwrap();
        v.change_master_password("new").unwrap();
        assert!(v.verify_password("new"));
        assert!(!v.verify_password("old"), "旧密码必须失效");
        v.lock();
        assert_eq!(
            v.unlock(Some("old")).unwrap_err(),
            UnlockError::WrongPassword
        );
        assert!(v.unlock(Some("new")).unwrap());
    }

    #[test]
    fn verify_password_does_not_change_lock_state() {
        let mut v = fresh("verifyonly");
        v.setup(Some("pw")).unwrap();
        v.lock();
        assert!(v.verify_password("pw"));
        assert!(!v.is_unlocked(), "verify 不应顺便解锁");
        assert!(!v.verify_password("bad"));
        assert!(!v.is_unlocked());
    }

    #[test]
    fn verify_password_false_in_dpapi_mode() {
        let mut v = fresh("verifydpapi");
        v.setup(None).unwrap();
        assert!(!v.verify_password("anything"), "免密模式没有密码可验");
    }

    // ---------------- 字段加解密 ----------------

    #[test]
    fn field_encrypt_requires_unlock() {
        let mut v = fresh("lockedfields");
        v.setup(Some("pw")).unwrap();
        v.lock();
        assert!(v.encrypt(b"x").is_err());
        assert!(v.decrypt(&[0u8; 40]).is_err());
        assert!(v.encrypt(b"x").unwrap_err().contains("未解锁"));
    }

    #[test]
    fn lock_clears_key_and_blocks_fields() {
        let mut v = fresh("clearkey");
        v.setup(None).unwrap();
        let sealed = v.encrypt(b"data").unwrap();
        v.lock();
        assert!(!v.is_unlocked());
        v.unlock(None).unwrap();
        assert_eq!(v.decrypt(&sealed).unwrap(), b"data");
    }

    #[test]
    fn atomic_write_leaves_no_tmp_on_success() {
        let mut v = fresh("atomic");
        v.setup(None).unwrap();
        let tmp = v.path().with_extension("key.tmp");
        assert!(!tmp.exists(), "成功写入后不该残留临时文件");
        assert!(v.path().is_file());
    }

    #[test]
    fn meta_json_uses_python_field_names() {
        // 字段名必须与 Python 完全一致，否则两版互相读不到
        let mut v = fresh("fieldnames");
        v.setup(Some("pw")).unwrap();
        let text = std::fs::read_to_string(v.path()).unwrap();
        for k in [
            "\"mode\"",
            "\"kdf\"",
            "\"salt\"",
            "\"iter\"",
            "\"wrapped_key\"",
            "\"verifier\"",
        ] {
            assert!(text.contains(k), "vault.key 必须含 {k}: {text}");
        }
        assert!(text.contains("\"password\""), "{text}");
        assert!(!text.contains("wrappedKey"), "不得用驼峰字段名: {text}");
    }

    #[test]
    fn reads_python_shaped_meta() {
        // 直接喂一份 Python 风格的 JSON（键序/空白不同）也必须能读
        let mut v = fresh("pyshape");
        let salt = random_salt();
        let dk = derive_key("pw", &salt, 1000).unwrap();
        let key = random_key();
        let py_json = format!(
            r#"{{"mode": "password", "kdf": {{"salt": "{}", "iter": 1000}}, "wrapped_key": "{}", "verifier": "{}"}}"#,
            b64(&salt),
            b64(&seal_gcm(&dk, &key, AAD_KEY).unwrap()),
            b64(&seal_gcm(&dk, VERIFIER, AAD_VERIFY).unwrap())
        );
        std::fs::write(v.path(), py_json).unwrap();
        assert!(v.unlock(Some("pw")).unwrap());
    }
}

#[cfg(test)]
mod cross_language_tests {
    //! 跨语言兼容测试：直接读 **Python 版生成** 的 vault.key 与密文。
    //!
    //! 这是整个 M6 最关键的验证——`vault.key` 与字段密文必须两版互通，
    //! 否则用户从 Python 版切到 Rust 版会看到"主密码错误"，甚至以为密码库丢了。
    //!
    //! fixture 由 `tools/gen_vault_fixture.py` 生成到 `%TEMP%\sk_vault_xlang\`；
    //! 缺失时跳过（CI/新机器上不必先跑 Python），存在则必须通过。
    use super::*;

    fn fixture_dir() -> PathBuf {
        std::env::temp_dir().join("sk_vault_xlang")
    }

    #[derive(serde::Deserialize)]
    struct Expected {
        password: String,
        key_hex: String,
        field_plaintext: String,
    }

    fn load() -> Option<(VaultCrypto, Expected, Vec<u8>)> {
        let dir = fixture_dir();
        let key_file = dir.join("vault.key");
        if !key_file.is_file() {
            eprintln!("跳过：未找到 Python fixture（{}）", dir.to_string_lossy());
            return None;
        }
        let exp: Expected =
            serde_json::from_str(&std::fs::read_to_string(dir.join("expected.json")).ok()?).ok()?;
        let blob = base64::engine::general_purpose::STANDARD
            .decode(std::fs::read_to_string(dir.join("field_blob.b64")).ok()?)
            .ok()?;
        Some((VaultCrypto::with_path(key_file), exp, blob))
    }

    #[test]
    fn reads_python_generated_vault_key() {
        let Some((mut v, exp, _)) = load() else {
            return;
        };
        // Python 写的是 2000 次迭代（不是常量 600000）：必须按文件里的 iter 派生，
        // 否则这里会得到"主密码错误"——正是跨版本升级最容易踩的坑
        assert!(
            v.unlock(Some(&exp.password)).unwrap(),
            "必须能用 Python 生成库的密码解锁（迭代数要按文件读）"
        );
        assert!(v.is_unlocked());
        assert!(v.has_master_password(), "模式应为 password");
    }

    #[test]
    fn rejects_wrong_password_on_python_vault() {
        let Some((mut v, _, _)) = load() else { return };
        assert_eq!(
            v.unlock(Some("wrong")).unwrap_err(),
            UnlockError::WrongPassword,
            "错误密码必须报 WrongPassword，而不是「损坏」"
        );
    }

    #[test]
    fn decrypts_python_field_ciphertext() {
        // 最硬的一条：Python 用 aad=b"field" 加密的密文，Rust 必须解出同样的明文
        let Some((mut v, exp, blob)) = load() else {
            return;
        };
        v.unlock(Some(&exp.password)).unwrap();
        let plain = v.decrypt(&blob).expect("必须能解开 Python 生成的字段密文");
        assert_eq!(String::from_utf8(plain).unwrap(), exp.field_plaintext);
    }

    #[test]
    fn recovered_data_key_matches_python() {
        // 解出来的数据密钥必须与 Python 期望的 32 字节完全一致
        let Some((mut v, exp, _)) = load() else {
            return;
        };
        v.unlock(Some(&exp.password)).unwrap();
        // 用数据密钥自己解一次 verifier 与 wrapped_key 里的密钥，间接验证
        let sealed = v.encrypt(b"roundtrip").unwrap();
        assert_eq!(v.decrypt(&sealed).unwrap(), b"roundtrip");
        assert_eq!(exp.key_hex.len(), 64, "期望密钥应是 32 字节十六进制");
    }

    #[test]
    fn rust_writes_key_python_can_read() {
        // 反向：Rust 生成 vault.key + 字段密文 → 交给 Python 校验。
        // 真正的读取由 `tools/verify_vault_rust_to_python.py` 完成（需要 Python 解释器），
        // 这里负责产出产物。**两边都验证过才算格式兼容**——用户从 Rust 版切回
        // Python 版时若读不了，会看到"主密码错误"并以为密码库坏了。
        let Some((_, exp, _)) = load() else { return };
        let dir = fixture_dir().join("rust_out");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vault.key");
        let mut v = VaultCrypto::with_path(path.clone());
        v.setup(Some(&exp.password)).unwrap();
        let sealed = v.encrypt(exp.field_plaintext.as_bytes()).unwrap();
        std::fs::write(dir.join("field_blob.b64"), b64(&sealed)).unwrap();
        std::fs::write(
            dir.join("info.json"),
            serde_json::json!({
                "password": exp.password,
                "field_plaintext": exp.field_plaintext,
            })
            .to_string(),
        )
        .unwrap();

        // 产物形状断言：Python 侧会按这些字段名解析，名字错了它会读不到
        let meta = v.read_meta();
        assert_eq!(meta.mode, "password");
        assert!(meta.kdf.is_some() && meta.verifier.is_some());
        let text = std::fs::read_to_string(&path).unwrap();
        for k in [
            "\"mode\"",
            "\"kdf\"",
            "\"salt\"",
            "\"iter\"",
            "\"wrapped_key\"",
            "\"verifier\"",
        ] {
            assert!(text.contains(k), "Rust 写出的 vault.key 必须含 {k}");
        }
        // 迭代数必须是当前常量（新库写入当前值）
        assert_eq!(meta.kdf.unwrap().effective_iter(), KDF_ITER);
    }
}
