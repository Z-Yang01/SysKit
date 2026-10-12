//! 敏感文件加密存储：DPAPI 封装的 JSON 容器（与 Python 侧 secure_store.py 逐字节对齐）。
//!
//! 容器格式：`{"v": 1, "enc": "dpapi", "data": "<base64(密文)>"}`
//! - 密封对象绑定当前 Windows 用户（CryptProtectData），换用户/换机器无法解密；
//! - 兼容旧明文文件（无 enc 字段）→ 返回原数据，下次 save 自动升级为密封格式；
//! - 密封损坏（换用户/截断）→ 返回 default（与旧版损坏行为一致）；
//! - 写入走原子替换；DPAPI 不可用等任何失败返回 false 并留痕。

use std::path::Path;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::paths;

pub const MARKER_V: i64 = 1;
pub const MARKER_ENC: &str = "dpapi";

/// 序列化 → DPAPI 密封 → 容器 JSON 文本（可直接写文件）。失败返回 None。
pub fn seal_json(data: &Value) -> Option<String> {
    let raw = serde_json::to_vec(data).ok()?;
    let sealed = crate::w32::dpapi_protect(&raw)?;
    let container = json!({
        "v": MARKER_V,
        "enc": MARKER_ENC,
        "data": B64.encode(sealed),
    });
    serde_json::to_string(&container).ok()
}

/// 读取密封文件返回 `(数据, 是否密封格式)`。
///
/// 与 Python load_sealed_json 同语义：文件不存在/损坏 → (default, false)；
/// enc=dpapi 但解密失败 → (default, false)；旧明文 → (原数据, false)。
pub fn load_sealed_json(path: &Path, default: Value) -> (Value, bool) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return (default, false),
    };
    let parsed: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return (default, false),
    };
    if let Some(obj) = parsed.as_object() {
        if obj.get("enc").and_then(Value::as_str) == Some(MARKER_ENC) {
            let b64 = obj.get("data").and_then(Value::as_str).unwrap_or("");
            let ok = B64
                .decode(b64)
                .ok()
                .and_then(|sealed| crate::w32::dpapi_unprotect(&sealed))
                .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
            return match ok {
                Some(v) => (v, true),
                None => {
                    log::warn!(
                        "密封数据解密失败（换用户/损坏），已回退默认值: {}",
                        paths::display(path)
                    );
                    (default, false)
                }
            };
        }
    }
    (parsed, false) // 旧明文：原样返回，下次保存自动密封
}

/// 密封写入（原子替换）。成功返回 true；任何失败（含 DPAPI 不可用）返回 false。
pub fn save_sealed_json(path: &Path, data: &Value) -> bool {
    let text = match seal_json(data) {
        Some(t) => t,
        None => {
            log::warn!("密封写入失败（DPAPI 不可用）: {}", paths::display(path));
            return false;
        }
    };
    match crate::jsonio::atomic_write(path, text.as_bytes()) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("密封写盘失败: {} ({e})", paths::display(path));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("syskit_rs_secure_{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn container_has_exactly_python_keys() {
        let text = seal_json(&json!([{"ts": 1, "text": "秘密"}])).expect("DPAPI 应可用");
        let v: Value = serde_json::from_str(&text).unwrap();
        let obj = v.as_object().unwrap();
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["data", "enc", "v"],
            "容器键必须与 Python 完全一致"
        );
        assert_eq!(obj["v"], json!(1));
        assert_eq!(obj["enc"], json!("dpapi"));
        // data 是标准 base64（含 padding），可解出非空密文
        let sealed = B64.decode(obj["data"].as_str().unwrap()).unwrap();
        assert!(!sealed.is_empty());

        // 逐字节信封形状：Python 用 json.dumps 默认分隔符（冒号后一个空格）。
        // 键顺序不构成契约（Python 保序、serde_json 按 BTreeMap 排序），解析只看键值。
        assert!(
            text.starts_with("{\"data\":\"") || text.starts_with("{\"enc\":\"dpapi\","),
            "信封必须是紧凑 JSON: {text}"
        );
        assert!(text.ends_with("v\":1}"), "信封必须以 v:1 收尾: {text}");
        assert_eq!(
            text.matches("\"data\":").count(),
            1,
            "data 字段只能出现一次"
        );
        // 紧凑格式：不得出现换行/缩进（Python json.dumps() 默认也不缩进）。
        // 注意不能数 ": " 出现次数——base64 字母表含 '+' 与 '/'，密文里可能
        // 恰好出现 ": " 序列，那种断言会随密文内容随机失败。
        assert!(!text.contains('\n'), "信封不得换行: {text}");
        assert!(!text.contains("  "), "信封不得缩进: {text}");
        assert_eq!(text.matches("{\"").count(), 1, "信封只有一个对象");
    }

    #[test]
    fn round_trip_sealed() {
        let d = temp_dir("rt");
        let p = d.join("clipboard.json");
        let data = json!([{"ts": 1700000000, "text": "中文内容", "kind": "text", "pinned": false, "data": ""}]);
        assert!(save_sealed_json(&p, &data));
        let (back, sealed) = load_sealed_json(&p, json!([]));
        assert!(sealed, "必须识别为密封格式");
        assert_eq!(back, data);
    }

    #[test]
    fn legacy_plaintext_is_read_as_is() {
        let d = temp_dir("legacy");
        let p = d.join("clipboard.json");
        std::fs::write(&p, r#"[{"ts":1,"text":"旧明文"}]"#).unwrap();
        let (back, sealed) = load_sealed_json(&p, json!([]));
        assert!(!sealed, "旧明文不应被当成密封格式");
        assert_eq!(back, json!([{"ts":1,"text":"旧明文"}]));
    }

    #[test]
    fn corrupt_sealed_returns_default() {
        let d = temp_dir("corrupt");
        let p = d.join("clipboard.json");
        std::fs::write(&p, r#"{"v":1,"enc":"dpapi","data":"bm90LWEtY2lwaGVy"}"#).unwrap();
        let (back, sealed) = load_sealed_json(&p, json!(["fallback"]));
        assert!(!sealed);
        assert_eq!(back, json!(["fallback"]));
    }

    #[test]
    fn missing_file_returns_default() {
        let d = temp_dir("missing");
        let (back, sealed) = load_sealed_json(&d.join("nope.json"), json!(null));
        assert!(!sealed);
        assert_eq!(back, json!(null));
    }
}
