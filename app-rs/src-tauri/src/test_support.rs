//! 测试支撑：把数据目录钉在临时目录，**保证单测绝不触碰真实用户数据**。
//!
//! 与 Python 侧 conftest 重定向 `APPDATA`/`LOCALAPPDATA` 的纪律同义：
//! `History::save()`、`settings::save_to()` 这些路径一旦落到 `%APPDATA%\SysKit`，
//! 就会污染用户既有的 settings.json / clipboard.json（两版共用该目录！）。
//!
//! 用 `Once` 保证并发测试只注入一次；`paths::set_appdata_override` 本身幂等。

#![cfg(test)]

use std::path::PathBuf;
use std::sync::Once;

static INIT: Once = Once::new();

/// 返回隔离的数据目录（首次调用时创建并注入）。
pub fn isolate() -> PathBuf {
    INIT.call_once(|| {
        let dir = std::env::temp_dir().join(format!("syskit_rs_testdata_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("无法创建测试数据目录");
        crate::paths::set_appdata_override(dir.clone());
    });
    crate::paths::appdata_dir()
}

/// 清空 settings 进程缓存（改过覆盖目录或写完盘之后重新读取用）。
pub fn reset_settings_cache() {
    crate::settings::reset_cache();
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
        // 幂等：再调一次还是同一个目录
        assert_eq!(isolate(), d);
    }

    #[test]
    fn clipboard_paths_are_inside_isolation() {
        let d = isolate();
        assert!(crate::paths::clipboard_file().starts_with(&d));
        assert!(crate::paths::settings_file().starts_with(&d));
        assert!(crate::paths::clipboard_files_dir().starts_with(&d));
    }
}
