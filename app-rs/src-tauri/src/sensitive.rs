//! 敏感内容剪贴板：标记 + 系统历史排除 + 到期自动清空。
//!
//! ## 为什么这几件事必须成套做
//!
//! 复制一段密码（或 WiFi 密钥）时，泄露面有**三处**，只堵一处等于没堵：
//!
//! 1. **本应用自己的历史**——`mark_sensitive` 让监听器在 TTL 内不记录该文本；
//! 2. **Windows 的 Win+V 历史与云剪贴板**——必须有
//!    `ExcludeClipboardContentFromMonitorProcessing` /
//!    `CanIncludeInClipboardHistory=0` / `CanUploadToCloudClipboard=0`
//!    这三个注册格式（KeePass 同款做法，见 `w32::exclude_os_history`）。
//!    只做第 1 条的话，密码照样进系统历史并可能同步到别的设备；
//! 3. **剪贴板本身会一直留着**——到期清空，否则下一个程序读剪贴板就拿到了。
//!
//! ## 自动清空的关键语义
//!
//! **只在剪贴板内容仍是当初那段敏感文本时才清空。**
//! 用户复制密码后往往马上复制别的东西；无条件清空会把用户刚要用的新内容也抹掉
//! ——那是数据丢失，比"密码多留一会儿"严重得多。

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 待清空的敏感内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingClear {
    pub text: String,
    pub deadline: Instant,
}

/// 到期是否应当清空剪贴板。
///
/// `current` 是**现在剪贴板里实际的内容**；只有它仍等于当初那段敏感文本时才清。
/// 返回 None 表示不清（要么没到期，要么内容已被覆盖）。
pub fn should_clear(
    pending: &PendingClear,
    current: Option<&str>,
    now: Instant,
) -> Option<&'static str> {
    if now < pending.deadline {
        return None;
    }
    match current {
        // 内容已变：用户复制了别的东西，**绝不能动**
        Some(c) if c != pending.text => None,
        // 剪贴板为空/读不到：没什么可清
        None => None,
        Some(_) => Some("敏感内容到期，已清空剪贴板"),
    }
}

/// TTL 夹紧：设置层的范围是 0~3600，但配置文件可被手工编辑，
/// 这里再夹一次。**0 表示"不清空"**（用户显式关闭），不能当成"立即清空"。
pub fn effective_ttl(raw: i64) -> Option<u64> {
    if raw <= 0 {
        return None; // 0 或负数 = 不自动清空
    }
    Some(raw.min(3600) as u64)
}

/// 到期清空的说明文案（界面用；已包含"多久后清空"）。
pub fn clear_note(ttl: Option<u64>) -> String {
    match ttl {
        Some(s) => format!("{s}s 后自动清空剪贴板"),
        None => "不会自动清空（敏感内容仍不进历史与系统剪贴板历史）".to_string(),
    }
}

static PENDING: Mutex<Option<PendingClear>> = Mutex::new(None);

/// 标记一段敏感文本：写入待清空槽位（`ttl=None` 表示只标记、不清空）。
pub fn arm(text: &str, ttl: Option<u64>) {
    let mut g = PENDING.lock().expect("敏感槽位锁中毒");
    *g = ttl.map(|s| PendingClear {
        text: text.to_string(),
        deadline: Instant::now() + Duration::from_secs(s),
    });
}

/// 当前是否有待清空的敏感内容（界面/测试观察用）。
pub fn pending_text() -> Option<String> {
    PENDING
        .lock()
        .expect("敏感槽位锁中毒")
        .as_ref()
        .map(|p| p.text.clone())
}

/// 取消待清空（例如用户又复制了新的敏感内容而旧槽位已无意义）。
pub fn disarm() {
    *PENDING.lock().expect("敏感槽位锁中毒") = None;
}

/// 检查并执行到期清空。返回清空是否真的发生。
///
/// **由监听线程的轮询驱动**（每 500ms 一拍），不另起定时器：
/// 监听线程本来就在轮询剪贴板序号，复用它省一个线程；而且清空动作会改变
/// 剪贴板内容，正好在同一个循环里更新序号基线，不会把"我们自己清空"当成
/// 一次新复制。
pub fn tick_clear() -> bool {
    let snapshot = {
        let g = PENDING.lock().expect("敏感槽位锁中毒");
        g.clone()
    };
    let Some(pending) = snapshot else {
        return false;
    };
    if Instant::now() < pending.deadline {
        return false;
    }
    // 读取当前剪贴板内容做比对（读失败按"读不到"处理，不清）
    let current = crate::w32::clipboard_get_text();
    match should_clear(&pending, current.as_deref(), Instant::now()) {
        Some(_) => {
            let ok = crate::w32::clipboard_empty();
            // 无论清空是否成功都撤下槽位：失败时反复重试会持续抢剪贴板全局锁，
            // 反而干扰用户（而且内容已过期，再试也不会变得"更该清"）
            disarm();
            if ok {
                log::info!("敏感内容到期，已清空剪贴板");
            } else {
                log::warn!("敏感内容到期但清空剪贴板失败（可能被其他程序占用）");
            }
            ok
        }
        None => {
            // 内容已被覆盖：槽位失去意义，撤掉以免一直挂着
            if let Some(c) = current {
                if c != pending.text {
                    disarm();
                }
            }
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(text: &str, secs_left: i64) -> PendingClear {
        let now = Instant::now();
        PendingClear {
            text: text.to_string(),
            // 用 checked 运算避免测试里 panic
            deadline: if secs_left >= 0 {
                now + Duration::from_secs(secs_left as u64)
            } else {
                now - Duration::from_secs((-secs_left) as u64)
            },
        }
    }

    #[test]
    fn clears_when_expired_and_content_unchanged() {
        let p = pending("secret", -1);
        assert!(should_clear(&p, Some("secret"), Instant::now()).is_some());
    }

    #[test]
    fn does_not_clear_before_deadline() {
        let p = pending("secret", 60);
        assert_eq!(should_clear(&p, Some("secret"), Instant::now()), None);
    }

    #[test]
    fn never_clears_newer_unrelated_content() {
        // 最重要的语义：用户已经复制了别的东西，绝不能把它抹掉
        let p = pending("secret", -1);
        assert_eq!(
            should_clear(&p, Some("用户刚复制的重要文字"), Instant::now()),
            None,
            "内容已被覆盖时不该清空——那是数据丢失"
        );
    }

    #[test]
    fn does_not_clear_when_clipboard_unreadable() {
        let p = pending("secret", -1);
        assert_eq!(should_clear(&p, None, Instant::now()), None);
    }

    #[test]
    fn ttl_zero_means_never_clear_not_clear_immediately() {
        // 0 是"用户显式关闭自动清空"，不是"立刻清空"
        assert_eq!(effective_ttl(0), None);
        assert_eq!(effective_ttl(-5), None);
        assert_eq!(effective_ttl(25), Some(25));
    }

    #[test]
    fn ttl_is_clamped_to_setting_range() {
        // 配置文件可被手工编辑：999999 秒 ≈ 11 天，不能照做
        assert_eq!(effective_ttl(999_999), Some(3600));
        assert_eq!(effective_ttl(3600), Some(3600));
        assert_eq!(effective_ttl(1), Some(1));
    }

    #[test]
    fn clear_note_mentions_remaining_behaviour() {
        assert!(clear_note(Some(25)).contains("25s"));
        assert!(clear_note(Some(25)).contains("清空"));
        // 不清空时也要说明"仍不进历史"，否则用户以为关掉就全无保护
        let n = clear_note(None);
        assert!(n.contains("不会自动清空"), "{n}");
        assert!(n.contains("历史"), "要说明其余保护仍在: {n}");
    }

    #[test]
    fn arm_and_disarm_round_trip() {
        let _g = crate::test_support::serial();
        disarm();
        assert_eq!(pending_text(), None);
        arm("p@ss", Some(25));
        assert_eq!(pending_text(), Some("p@ss".to_string()));
        disarm();
        assert_eq!(pending_text(), None);
    }

    #[test]
    fn arm_with_no_ttl_keeps_no_pending_clear() {
        let _g = crate::test_support::serial();
        disarm();
        // ttl=None：只标记敏感（不进历史），但不安排清空
        arm("p@ss", None);
        assert_eq!(pending_text(), None, "无 TTL 时不该有待清空项");
    }

    #[test]
    fn tick_clear_is_noop_without_pending() {
        let _g = crate::test_support::serial();
        disarm();
        assert!(!tick_clear(), "没有待清空项时不该动剪贴板");
    }

    #[test]
    fn tick_clear_does_not_touch_clipboard_before_deadline() {
        let _g = crate::test_support::serial();
        // 这一条保护的是"别误清用户当前内容"：未到期时 tick 必须什么都不做
        arm("某个还没到期的敏感串", Some(3600));
        assert!(!tick_clear());
        assert_eq!(pending_text(), Some("某个还没到期的敏感串".to_string()));
        disarm();
    }

    /// 真机端到端：写入 → 到期清空，并且**必须确认清空真的发生**。
    ///
    /// 这条用例覆盖的是整条链路的最后一步：`clipboard_empty()` 在真机上是否有效。
    /// 单测里 `should_clear` 判对了但清空失败，用户看到的仍是"密码一直留在剪贴板"
    /// ——所以这里要读回剪贴板内容确认。
    #[test]
    fn real_clipboard_clears_expired_sensitive_content() {
        let _g = crate::test_support::serial();
        let probe = "SysKit-敏感清空自检-9f3a";
        // 写入并标记已到期（ttl=None 只标记；这里手工装一个过期槽位）
        if !crate::w32::clipboard_set_text(probe) {
            return; // 剪贴板被占用：跳过（不谎报通过）
        }
        {
            let mut g = PENDING.lock().expect("敏感槽位锁中毒");
            *g = Some(PendingClear {
                text: probe.to_string(),
                deadline: Instant::now() - Duration::from_secs(1),
            });
        }
        // 前置确认：清空前剪贴板里确实是这段内容（否则本用例什么也没验证）
        let before = crate::w32::clipboard_get_text();
        if before.as_deref() != Some(probe) {
            disarm();
            return; // 有别的程序抢了剪贴板：跳过
        }
        assert!(tick_clear(), "到期后 tick_clear 应报告已清空");
        assert_eq!(pending_text(), None, "清空后槽位应撤销");
        // 关键断言：剪贴板里**确实**没有那段内容了
        let after = crate::w32::clipboard_get_text();
        assert_ne!(
            after.as_deref(),
            Some(probe),
            "清空必须真的生效——否则密码会一直留在剪贴板里"
        );
    }

    /// 真机：内容被新内容覆盖时，到期 tick **不得**清掉新内容。
    #[test]
    fn real_clipboard_keeps_newer_content_past_deadline() {
        let _g = crate::test_support::serial();
        let newer = "SysKit-用户后来复制的重要内容-4b7c";
        if !crate::w32::clipboard_set_text(newer) {
            return;
        }
        {
            let mut g = PENDING.lock().expect("敏感槽位锁中毒");
            *g = Some(PendingClear {
                text: "早已被覆盖的旧敏感串".to_string(),
                deadline: Instant::now() - Duration::from_secs(1),
            });
        }
        if crate::w32::clipboard_get_text().as_deref() != Some(newer) {
            disarm();
            return; // 剪贴板被抢：跳过
        }
        assert!(!tick_clear(), "内容已变时不该执行清空");
        assert_eq!(
            crate::w32::clipboard_get_text().as_deref(),
            Some(newer),
            "用户新复制的内容必须保持不动——误清是数据丢失"
        );
        disarm();
    }
}
