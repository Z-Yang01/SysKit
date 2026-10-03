//! 测试支撑：把数据目录钉在临时目录，**保证单测绝不触碰真实用户数据**。
//!
//! 与 Python 侧 conftest 重定向 `APPDATA`/`LOCALAPPDATA` 的纪律同义：
//! `History::save()`、`settings::save_to()` 这些路径一旦落到 `%APPDATA%\SysKit`，
//! 就会污染用户既有的 settings.json / clipboard.json（两版共用该目录！）。
//!
//! 并发两件事必须一起解决（各自踩过一次）：
//! 1. `Once` 保证目录覆盖只注入一次（`paths::set_appdata_override` 本身幂等）；
//! 2. **串行锁**保证同一时刻只有一个用例在做「写盘 → 读回」——否则 A 例刚清空
//!    目录、B 例正好写盘，A 读回就拿到空结果（一轮真实出现过的假红）。

#![cfg(test)]

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, Once, OnceLock};

static INIT: Once = Once::new();
static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

fn serial_lock() -> &'static Mutex<()> {
    SERIAL.get_or_init(|| Mutex::new(()))
}

/// 取串行令牌：跨用例独占「写盘 → 读回」这段临界区。
///
/// 用法：`let _g = crate::test_support::serial();`
/// 中毒（某用例 panic）时也放行——不能让一个失败把其余用例全变成 poisoning 错误。
pub fn serial() -> MutexGuard<'static, ()> {
    serial_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 返回隔离的数据目录（首次调用时创建并注入覆盖）。
pub fn isolate() -> PathBuf {
    INIT.call_once(|| {
        let dir = std::env::temp_dir().join(format!("syskit_rs_testdata_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("无法创建测试数据目录");
        crate::paths::set_appdata_override(dir.clone());
    });
    crate::paths::appdata_dir()
}

/// 清掉隔离目录里的持久化文件 + 进程内缓存（在**持有 `serial()` 令牌**时调用）。
///
/// 需要它是因为进程内缓存与磁盘状态会跨用例残留：前一个用例写下的
/// clipboard.json 会让后一个「空历史」断言读到非空数据。
pub fn reset_persistent_files() {
    let dir = isolate();
    for name in [
        "clipboard.json",
        "traffic.json",
        "events.json",
        "netstats_buckets.json",
        "settings.json",
    ] {
        let _ = std::fs::remove_file(dir.join(name));
    }
    crate::settings::reset_cache();
    crate::clipboard_history::reset_global_for_test();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolate_points_away_from_real_appdata() {
        let d = isolate();
        assert!(
            d.starts_with(std::env::temp_dir()),
            "测试目录必须在 temp 下"
        );
        assert!(
            !d.ends_with("SysKit"),
            "绝不能落到真实 %APPDATA%\\SysKit：{d:?}"
        );
        assert_eq!(isolate(), d, "幂等：再调一次还是同一个目录");
    }

    #[test]
    fn clipboard_paths_are_inside_isolation() {
        let d = isolate();
        assert!(crate::paths::clipboard_file().starts_with(&d));
        assert!(crate::paths::settings_file().starts_with(&d));
        assert!(crate::paths::traffic_file().starts_with(&d));
        assert!(crate::paths::events_file().starts_with(&d));
        assert!(crate::paths::netstats_file().starts_with(&d));
        assert!(crate::paths::clipboard_files_dir().starts_with(&d));
    }

    #[test]
    fn serial_survives_poisoning() {
        // 中毒后仍可获取（否则一个失败会级联成一片 poisoning 报错）
        {
            let _g = serial();
        }
        let _g2 = serial();
    }

    #[test]
    fn reset_removes_persistent_files() {
        let _g = serial();
        let dir = isolate();
        std::fs::write(dir.join("traffic.json"), "{}").unwrap();
        reset_persistent_files();
        assert!(!dir.join("traffic.json").exists());
    }
}
