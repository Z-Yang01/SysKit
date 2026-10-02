//! 原子 JSON 读写：与 Python 侧 stores.py 的写入纪律对齐。
//!
//! 写入一律走「临时文件 + os.replace 等价物」：断电/崩溃只会留下完整旧文件
//! 或完整新文件，不会出现半截 JSON。损坏回退默认值并留痕。

use std::path::{Path, PathBuf};

/// 原子写入：写 `<path>.tmp` 后替换；父目录自动创建。
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let tmp = tmp_path(path);
    match std::fs::write(&tmp, bytes) {
        Ok(()) => {}
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

/// 读取 JSON；不存在/损坏返回 `default`（并留痕），语义等同 stores._load。
pub fn load_json(path: &Path, default: serde_json::Value) -> serde_json::Value {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("JSON 损坏，已回退默认值: {} ({e})", path.display());
                default
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => default,
        Err(e) => {
            log::warn!("JSON 读取失败，已回退默认值: {} ({e})", path.display());
            default
        }
    }
}

/// 序列化写入 JSON（紧凑格式，对齐 Python `json.dump` 默认分隔符）。
pub fn save_json(path: &Path, data: &serde_json::Value) -> bool {
    match serde_json::to_string(data) {
        Ok(text) => {
            if let Err(e) = atomic_write(path, text.as_bytes()) {
                log::warn!("JSON 写盘失败: {} ({e})", path.display());
                return false;
            }
            true
        }
        Err(e) => {
            log::warn!("JSON 序列化失败: {} ({e})", path.display());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("syskit_rs_stores_{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn round_trip_and_no_tmp_left() {
        let d = temp_dir("rt");
        let p = d.join("nested").join("data.json");
        assert!(save_json(&p, &json!({"a": 1, "b": "中文"})));
        assert_eq!(load_json(&p, json!(null)), json!({"a": 1, "b": "中文"}));
        assert!(!tmp_path(&p).exists(), "临时文件必须已被替换掉");
    }

    #[test]
    fn missing_file_returns_default() {
        let d = temp_dir("missing");
        assert_eq!(load_json(&d.join("nope.json"), json!([])), json!([]));
    }

    #[test]
    fn corrupt_file_returns_default() {
        let d = temp_dir("corrupt");
        let p = d.join("bad.json");
        std::fs::write(&p, "{ 半截").unwrap();
        assert_eq!(load_json(&p, json!("fallback")), json!("fallback"));
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let d = temp_dir("replace");
        let p = d.join("a.json");
        assert!(save_json(&p, &json!({"v": 1})));
        assert!(save_json(&p, &json!({"v": 2})));
        assert_eq!(load_json(&p, json!(null)), json!({"v": 2}));
    }
}
