//! M6 密码库命令：会话管理（解锁/重锁/信任窗）+ 数据层薄封装 + 密码生成。
use super::copy_sensitive_text;

// ---------------- M6 密码库（会话式：解锁后数据命令共享同一实例） ----------------

/// 会话持有解锁后的 VaultCrypto；None = 未解锁。
/// 旧实现每个命令各 new 一个实例——unlock 与数据命令不在同一实例上，
/// 数据密钥根本传不过去（M6 接线时修正为托管会话）。
pub struct VaultSession(pub std::sync::Mutex<Option<crate::vault_crypto::VaultCrypto>>);

/// 主密码免重复输入窗口的到期时刻（sk_vault_verify_password 成功时续期）。
pub struct VaultTrust(pub std::sync::Mutex<Option<std::time::Instant>>);

pub(crate) fn session_lock(
    session: &VaultSession,
) -> Result<std::sync::MutexGuard<'_, Option<crate::vault_crypto::VaultCrypto>>, String> {
    // 库表确保存在：所有 vault 命令都从这里进——曾经只有 sk_vault_sites 调了
    // init_db，"设好主密码→直接添加站点"会撞 no such table: sites（真机实证
    // 2026-10-04）。init_db 幂等且带进程级短路，放在公共入口零遗漏。
    crate::vault_store::init_db().map_err(|e| format!("密码库初始化失败: {e}"))?;
    // 自动重锁：vault_autolock_min>0 且**闲置**超过窗口 → 就地收锁。
    // 「闲置」= 距上次密码库操作（unlocked_at 会被每次放行的操作 touch 刷新），
    // 与 Python 侧 _ensure_unlocked → _restart_autolock 同口径——按"解锁那一刻"
    // 起算的话，连续操作的用户也会被中途踢出，TOTP 轮询显示会反复被
    // 「密码库未解锁」打断（轮次36 跨模块审计实锤）。
    {
        let mut guard = session.0.lock().expect("vault 会话锁中毒");
        let min = crate::settings::get_int("vault_autolock_min");
        if let (Some(c), true) = (guard.as_mut(), min > 0) {
            if let Some(since) = c.unlocked_since_secs() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                if vault_autolock_due(now - since, min) {
                    c.lock();
                    *guard = None;
                }
            }
        }
        // 活跃续期：本次操作放行后把闲置计时归零（下次操作的 due 判定以此为基准）
        if let Some(c) = guard.as_mut() {
            c.touch_unlocked();
        }
    }
    let guard = session.0.lock().expect("vault 会话锁中毒");
    if guard.is_none() {
        return Err("密码库未解锁".into());
    }
    Ok(guard)
}

/// 自动重锁到期判定（纯函数，供单测/变异验证）：闲置秒数达到 `min` 分钟。
/// `min<=0` 恒不锁定（设置 0 = 不自动锁）；边界取 `>=`（与 monitor 冷却同风格）。
pub(crate) fn vault_autolock_due(idle_secs: f64, min: i64) -> bool {
    min > 0 && idle_secs >= min as f64 * 60.0
}

/// 取会话守卫（未解锁 → Err）。调用方以 `guard.as_mut().expect(...)` 拿实例——
/// MutexGuard::map 尚未稳定（mapped_lock_guards），不走它。
fn session_get(
    session: &VaultSession,
) -> Result<std::sync::MutexGuard<'_, Option<crate::vault_crypto::VaultCrypto>>, String> {
    session_lock(session)
}

/// DPAPI 免密模式专用：状态命令里把自动解锁的实例复制进会话
/// （本实例继续读 meta；数据密钥是随机 32B，重解锁无副作用）。
fn reopen_unlocked(source: &crate::vault_crypto::VaultCrypto) -> crate::vault_crypto::VaultCrypto {
    let mut n = crate::vault_crypto::VaultCrypto::with_path(source.path().to_path_buf());
    let _ = n.unlock(None);
    n
}

/// 密码库状态（含各能力的可用性，供界面如实展示）。
#[tauri::command]
pub fn sk_vault_status(
    session: tauri::State<'_, VaultSession>,
    trust: tauri::State<'_, VaultTrust>,
) -> serde_json::Value {
    let mut c = crate::vault_crypto::VaultCrypto::new();
    let path = c.path().to_string_lossy().to_string();
    let provisioned = c.is_provisioned();
    let meta = c.read_meta();
    let has_pw = provisioned && meta.mode == "password";
    // 免密模式可以自动解锁（DPAPI 绑定当前用户，无需输入）
    if provisioned && meta.mode == "dpapi" && c.unlock(None) == Ok(true) {
        // 自动解锁后的实例进会话：数据命令立即可用
        let auto = reopen_unlocked(&c);
        *session.0.lock().expect("vault 会话锁中毒") = Some(auto);
    }
    let session_unlocked = session.0.lock().expect("vault 会话锁中毒").is_some();
    let trusted_until = trust.0.lock().expect("vault trust 锁中毒").map(|t| {
        t.checked_duration_since(std::time::Instant::now())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    });
    serde_json::json!({
        "path": path,
        "provisioned": provisioned,
        "unlocked": session_unlocked,
        "has_master_password": has_pw,
        "mode": meta.mode,
        // KDF 算法（"pbkdf2" | "argon2id"；DPAPI/未初始化为 null）——加字段向后兼容，
        // 前端检测 argon2id 时提示「回退 v1.2.0 将无法打开」
        "kdf": c.kdf_algo(),
        "trusted_until": trusted_until,
        // 各能力可用性：界面据此决定按钮能不能点、要不要显示说明
        "capabilities": {
            "key_management": true,
            "password_storage": true,
        },
    })
}

/// 初始化密码库（首次创建 `vault.key`）。`password` 为空则用 DPAPI 免密模式。
#[tauri::command]
pub async fn sk_vault_setup(
    session: tauri::State<'_, VaultSession>,
    password: Option<String>,
) -> Result<serde_json::Value, String> {
    // 600k 次 PBKDF2 在主线程会明显卡顿，所以走阻塞线程池；
    // State 借用不能进 'static 闭包——本地实例跑完再进会话
    let (c, mode) = tauri::async_runtime::spawn_blocking(move || {
        let mut c = crate::vault_crypto::VaultCrypto::new();
        if c.is_provisioned() {
            return Err(("密码库已初始化".to_string(), "dpapi"));
        }
        let pw = password.filter(|p| !p.is_empty());
        let mode = if pw.is_some() { "password" } else { "dpapi" };
        c.setup(pw.as_deref()).map_err(|e| (e, mode))?;
        Ok((c, mode))
    })
    .await
    .map_err(|e| format!("初始化线程失败: {e}"))?
    .map_err(|(e, _)| e)?;
    *session.0.lock().expect("vault 会话锁中毒") = Some(c);
    Ok(serde_json::json!({
        "ok": true,
        "mode": mode,
        "message": if mode == "password" {
            "已创建密码库（主密码模式）"
        } else {
            "已创建密码库（免密模式：密钥由 Windows DPAPI 绑定当前用户保护）"
        },
    }))
}

/// 解锁。失败返回可读原因（区分密码错误与文件损坏）。
#[tauri::command]
pub async fn sk_vault_unlock(
    session: tauri::State<'_, VaultSession>,
    password: Option<String>,
) -> Result<serde_json::Value, String> {
    let unlocked = tauri::async_runtime::spawn_blocking(move || {
        let mut c = crate::vault_crypto::VaultCrypto::new();
        match c.unlock(password.as_deref()) {
            Ok(true) => Ok(c),
            Ok(false) => Err("解锁失败".to_string()),
            Err(e) => Err(e.message()),
        }
    })
    .await
    .map_err(|e| format!("解锁线程失败: {e}"))?;
    let c = unlocked?;
    *session.0.lock().expect("vault 会话锁中毒") = Some(c);
    Ok(serde_json::json!({ "ok": true, "message": "已解锁" }))
}

/// 锁定（清空内存中的密钥）。
#[tauri::command]
pub fn sk_vault_lock(session: tauri::State<VaultSession>) -> serde_json::Value {
    if let Some(mut c) = session.0.lock().expect("vault 会话锁中毒").take() {
        c.lock();
    }
    serde_json::json!({ "ok": true, "message": "已锁定" })
}

/// 设置主密码（免密 → 密码模式；数据密钥不变）。
#[tauri::command]
pub async fn sk_vault_set_password(
    session: tauri::State<'_, VaultSession>,
    password: String,
) -> Result<String, String> {
    // 会话实例先 take 出来（重挂 600k PBKDF2 期间数据命令短暂不可用，可接受），
    // 避免把 State 借用带进 'static 阻塞闭包
    let mut c = session
        .0
        .lock()
        .expect("vault 会话锁中毒")
        .take()
        .ok_or_else(|| "密码库未解锁".to_string())?;
    let result =
        tauri::async_runtime::spawn_blocking(move || match c.set_master_password(&password) {
            Ok(()) => Ok(c),
            Err(e) => Err((c, e)),
        })
        .await
        .map_err(|e| format!("设置线程失败: {e}"))?;
    // 无论成败都放回会话（实例仍持有解锁态）
    match result {
        Ok(c) => {
            *session.0.lock().expect("vault 会话锁中毒") = Some(c);
            Ok("已设置主密码（数据密钥未变，已有密文仍可解开）".to_string())
        }
        Err((c, e)) => {
            *session.0.lock().expect("vault 会话锁中毒") = Some(c);
            Err(e)
        }
    }
}

/// 校验主密码（不改变解锁状态）；成功续期免重复输入窗口。
#[tauri::command]
pub async fn sk_vault_verify_password(
    trust: tauri::State<'_, VaultTrust>,
    password: String,
) -> Result<bool, String> {
    let ok = tauri::async_runtime::spawn_blocking(move || {
        let c = crate::vault_crypto::VaultCrypto::new();
        c.verify_password(&password)
    })
    .await
    .map_err(|e| format!("校验线程失败: {e}"))?;
    if ok {
        let mins = crate::settings::get_int("vault_trust_min").max(0) as u64;
        let mut t = trust.0.lock().expect("vault trust 锁中毒");
        *t = if mins > 0 {
            Some(std::time::Instant::now() + std::time::Duration::from_secs(mins * 60))
        } else {
            None
        };
    }
    Ok(ok)
}

/// 移除主密码（回到免密模式）。当前密码错误返回 Ok(false)。
#[tauri::command]
pub async fn sk_vault_remove_password(
    session: tauri::State<'_, VaultSession>,
    current: String,
) -> Result<bool, String> {
    let mut c = session
        .0
        .lock()
        .expect("vault 会话锁中毒")
        .take()
        .ok_or_else(|| "密码库未解锁".to_string())?;
    let (c, result) = tauri::async_runtime::spawn_blocking(move || {
        let r = c.remove_master_password(&current);
        (c, r)
    })
    .await
    .map_err(|e| format!("移除线程失败: {e}"))?;
    *session.0.lock().expect("vault 会话锁中毒") = Some(c);
    result
}

// ---------------- M6 数据层命令（vault_store 薄封装；会话必须已解锁） ----------------

#[tauri::command]
pub fn sk_vault_sites(
    session: tauri::State<'_, VaultSession>,
    q: String,
) -> Result<serde_json::Value, String> {
    let _session = session_lock(&session)?;
    crate::vault_store::init_db().map_err(|e| e.to_string())?;
    let sites = crate::vault_store::search_sites(&q).map_err(|e| e.to_string())?;
    serde_json::to_value(sites).map_err(|e| e.to_string())
}

/// 重复密码检测：全表解密 password 列（GCM 随机 nonce 无密文捷径），明文
/// 在本函数内比对聚合、**掩码后**返回——明文不出后端。显式触发（安全体检
/// 按钮），组数上限 50；耗时打日志供后续决定是否要分页优化。
#[tauri::command]
pub fn sk_vault_reused_passwords(
    session: tauri::State<'_, VaultSession>,
) -> Result<serde_json::Value, String> {
    let guard = session_lock(&session)?;
    let c = guard.as_ref().ok_or("密码库未解锁")?;
    let started = std::time::Instant::now();
    let rows = crate::vault_store::all_accounts_with_passwords()
        .map_err(|e| format!("读取账号失败: {e}"))?;
    let mut groups: std::collections::HashMap<String, Vec<serde_json::Value>> =
        std::collections::HashMap::new();
    let mut weak: Vec<serde_json::Value> = Vec::new();
    let mut decrypted = 0usize;
    for (_id, site, label, enc) in rows {
        let Some(enc) = enc.filter(|e| !e.is_empty()) else {
            continue;
        };
        let Ok(pw) = c.decrypt(&enc) else {
            continue; // 单条解密失败降级跳过（与 bulk 同语义）
        };
        let Ok(pw) = String::from_utf8(pw) else {
            continue;
        };
        if pw.is_empty() {
            continue;
        }
        decrypted += 1;
        // 弱密码：单次遍历同轮判定（同条密码可同时出现在重复组与弱密码组）
        if let Some(reason) = weak_reason(&pw) {
            weak.push(serde_json::json!({
                "masked": mask_password(&pw),
                "reason": reason,
                "site": site,
                "label": label,
            }));
        }
        groups.entry(pw).or_default().push(serde_json::json!({
            "site": site,
            "label": label,
        }));
    }
    weak.sort_by_key(|w| w["site"].as_str().unwrap_or("").to_string());
    let reused: Vec<serde_json::Value> = groups
        .into_iter()
        .filter(|(_, accs)| accs.len() > 1)
        .map(|(pw, accs)| serde_json::json!({ "masked": mask_password(&pw), "accounts": accs }))
        .take(50)
        .collect();
    log::info!(
        "重复密码检测：解密 {decrypted} 条，耗时 {:?}",
        started.elapsed()
    );
    let weak_count = weak.len();
    Ok(serde_json::json!({
        "groups": reused,
        "count": reused.len(),
        "weak": weak.into_iter().take(100).collect::<Vec<_>>(),
        "weak_count": weak_count,
    }))
}

/// 弱密码判定（与前端 vault.ts passwordStrength 同口径；明文不出后端，
/// 只回掩码 + 原因）。规则刻意保守：长度优先，复杂度其次。
pub(crate) fn weak_reason(pw: &str) -> Option<&'static str> {
    if pw.chars().count() < 8 {
        return Some("少于 8 位");
    }
    let has_upper = pw.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = pw.chars().any(|c| c.is_ascii_lowercase());
    let has_digit = pw.chars().any(|c| c.is_ascii_digit());
    let has_symbol = pw.chars().any(|c| !c.is_ascii_alphanumeric());
    let kinds = [has_upper, has_lower, has_digit, has_symbol]
        .iter()
        .filter(|x| **x)
        .count();
    if pw.chars().count() < 12 && kinds < 2 {
        return Some("字符种类单一");
    }
    None
}

/// 密码掩码：前2 + **** + 后2；长度 <6 一律 `****`（防掩码等于原文）。
pub(crate) fn mask_password(pw: &str) -> String {
    let chars: Vec<char> = pw.chars().collect();
    if chars.len() < 6 {
        return "****".to_string();
    }
    let head: String = chars[..2].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("{head}****{tail}")
}

#[tauri::command]
pub fn sk_vault_site_accounts(
    session: tauri::State<'_, VaultSession>,
    site_id: i64,
    q: String,
) -> Result<serde_json::Value, String> {
    let _session = session_lock(&session)?;
    let accs = crate::vault_store::list_accounts(site_id, &q).map_err(|e| e.to_string())?;
    serde_json::to_value(accs).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_add_site(
    session: tauri::State<'_, VaultSession>,
    name: String,
    domain: String,
    url: String,
    aliases: String,
    tags: String,
) -> Result<i64, String> {
    let _session = session_lock(&session)?;
    crate::vault_store::add_site(&name, &domain, &url, &aliases, &tags).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_update_site(
    session: tauri::State<'_, VaultSession>,
    site_id: i64,
    name: Option<String>,
    domain: Option<String>,
    url: Option<String>,
    aliases: Option<String>,
    tags: Option<String>,
) -> Result<(), String> {
    let _session = session_lock(&session)?;
    crate::vault_store::update_site(
        site_id,
        &crate::vault_store::SitePatch {
            name,
            domain,
            url,
            aliases,
            tags,
        },
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_delete_site(
    session: tauri::State<'_, VaultSession>,
    site_id: i64,
) -> Result<(), String> {
    let _session = session_lock(&session)?;
    crate::vault_store::delete_site(site_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_add_account(
    session: tauri::State<'_, VaultSession>,
    site_id: i64,
    password: String,
    label: String,
    username: String,
    totp_secret: String,
    notes: String,
) -> Result<i64, String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    crate::vault_store::add_account(
        site_id,
        &password,
        crypto,
        &label,
        &username,
        &totp_secret,
        &notes,
    )
}

#[tauri::command]
pub fn sk_vault_update_account(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
    label: Option<String>,
    username: Option<String>,
    password: Option<String>,
    totp_secret: Option<String>,
    notes: Option<String>,
) -> Result<(), String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    crate::vault_store::update_account(
        account_id,
        crypto,
        &crate::vault_store::AccountPatch {
            label,
            username,
            password,
            totp_secret,
            notes,
        },
    )
}

#[tauri::command]
pub fn sk_vault_delete_account(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
) -> Result<(), String> {
    let _session = session_lock(&session)?;
    crate::vault_store::delete_account(account_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_toggle_favorite(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
) -> Result<bool, String> {
    let _session = session_lock(&session)?;
    crate::vault_store::toggle_favorite(account_id).map_err(|e| e.to_string())
}

/// 解密单字段。`sensitive_ttl` 给定时同步登记敏感窗口（复制密码/TOTP 用，
/// 与 Python 版 UI 同口径：TTL 内剪贴板监听不记录 + 系统侧排除）。
#[tauri::command]
pub fn sk_vault_get_secret(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
    field: String,
    touch: Option<bool>,
) -> Result<String, String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    crate::vault_store::get_secret(account_id, crypto, &field, touch.unwrap_or(true))
}

/// 解密并复制（明文不过前端 JS；三重敏感防护与复制命令同一实现）。
#[tauri::command]
pub fn sk_vault_copy_secret(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
    field: String,
) -> Result<serde_json::Value, String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    let text = crate::vault_store::get_secret(account_id, crypto, &field, true)?;
    copy_sensitive_text(&text)
}

/// 明文复制（用户名等非敏感字段；走系统剪贴板，无敏感防护）。
#[tauri::command]
pub fn sk_clip_copy_plain(text: String) -> Result<(), String> {
    if text.is_empty() {
        return Err("内容为空".to_string());
    }
    if crate::w32::clipboard_set_text(&text) {
        Ok(())
    } else {
        Err("写入剪贴板失败（可能被其他程序占用）".to_string())
    }
}

/// 批量解密（整列显示语义；单条损坏降级缺席，不外溢）。
#[tauri::command]
pub fn sk_vault_secrets_bulk(
    session: tauri::State<'_, VaultSession>,
    account_ids: Vec<i64>,
    field: String,
) -> Result<serde_json::Value, String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    let map = crate::vault_store::get_secrets_bulk(&account_ids, crypto, &field)?;
    serde_json::to_value(map).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_global_search(
    session: tauri::State<'_, VaultSession>,
    q: String,
) -> Result<serde_json::Value, String> {
    let _session = session_lock(&session)?;
    let rows = crate::vault_store::global_search(&q).map_err(|e| e.to_string())?;
    serde_json::to_value(rows).map_err(|e| e.to_string())
}

/// 现算 TOTP 验证码（密文按需解密；验证码本身不进敏感窗口）。
#[tauri::command]
pub fn sk_vault_totp_code(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
) -> Result<String, String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    let secret = crate::vault_store::get_secret(account_id, crypto, "totp", false)?;
    Ok(crate::vault_store::totp_code(&secret, None, 6, 30))
}

/// 生成随机密码（表单「生成」按钮）。参数与语义见 pwgen 模块头注释：
/// 拒绝采样保证均匀、每个启用类保底一个、参数不对（长度越界/未启用任何
/// 字符类）返回 Err 由前端提示。
///
/// 不依赖密码库会话——生成是纯 CSPRNG 运算，不触碰任何密文。
/// **纪律**：生成的密码在用户点「保存」前只活在表单输入框里，但它已经是
/// 凭据材料（暴露窗口与已保存密码一致）——此时复制必须走
/// `sk_clip_copy_sensitive` 敏感通道（不进本应用历史 / 不进 Win+V 与云剪贴板 /
/// 到期清空），与 `vault::password_copy_routes_through_sensitive` 守卫维持的
/// 密码类口径相同；前端源码守卫 tests/vault.test.ts 锁死这一接线。
#[tauri::command]
pub fn sk_vault_generate_password(
    length: usize,
    upper: bool,
    lower: bool,
    digits: bool,
    symbols: bool,
    exclude_ambiguous: bool,
) -> Result<String, String> {
    crate::pwgen::generate(crate::pwgen::GenOptions {
        length,
        upper,
        lower,
        digits,
        symbols,
        exclude_ambiguous,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weak_reason_matches_frontend_rules() {
        // 长度优先
        assert_eq!(weak_reason("Ab1!"), Some("少于 8 位"));
        assert_eq!(weak_reason("1234567"), Some("少于 8 位"));
        // 8~11 位且字符种类单一 → 弱
        assert_eq!(weak_reason("abcdefghij"), Some("字符种类单一"));
        assert_eq!(weak_reason("1234567890"), Some("字符种类单一"));
        // 8+ 位两种类以上 → 过
        assert_eq!(weak_reason("abcdefg1"), None);
        assert_eq!(weak_reason("Abcdefg1"), None);
        // 12+ 位单一种类也过（长度补偿复杂度）
        assert_eq!(weak_reason("aaaaaaaaaaaa"), None);
        // 符号计入种类
        assert_eq!(weak_reason("abcdefg!"), None);
    }
}
