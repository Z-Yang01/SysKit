//! 设置持久化：与 Python 侧 settings.py 同语义（DEFAULTS + 类型收敛 + 原子写）。
//!
//! 为什么必须有类型收敛这层：文件半截 JSON 有兜底，但「合法 JSON + 错误类型」
//! （手改/被其他工具写坏，如 poll_interval_sec="很快"）会原样穿透到消费方。
//! 只校验 DEFAULTS 已知键：last_page / modes_prev_volume 等动态键原样保留。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde_json::{Map, Value};

use crate::paths;

/// DEFAULTS 的默认值形态：决定类型收敛的目标类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Bool,
    Int,
    Str,
}

/// 已知设置键 → (默认值, 类型)。逐键对齐 config.py::DEFAULTS（32 个键）。
pub fn defaults() -> Vec<(&'static str, Value, Ty)> {
    use Ty::{Bool, Int, Str};
    vec![
        ("auto_repair_enabled", Value::Bool(true), Bool),
        ("poll_interval_sec", Value::from(30), Int),
        ("auto_repair_max_level", Value::from(3), Int),
        ("fail_threshold", Value::from(2), Int),
        ("cooldown_sec", Value::from(180), Int),
        ("level3_cooldown_sec", Value::from(300), Int),
        ("minimize_to_tray", Value::Bool(true), Bool),
        ("tray_rate_enabled", Value::Bool(true), Bool),
        ("sound_alert_enabled", Value::Bool(true), Bool),
        ("dark_mode", Value::Bool(false), Bool),
        ("dns_preset", Value::from("阿里"), Str),
        ("clipboard_max", Value::from(50), Int),
        ("clipboard_watch", Value::Bool(false), Bool),
        ("clipboard_retain_days", Value::from(1), Int),
        (
            "clip_process_blacklist",
            Value::from("keepass,bitwarden,1password,enpass"),
            Str,
        ),
        ("clip_sensitive_ttl", Value::from(25), Int),
        ("clip_capture_images", Value::Bool(true), Bool),
        ("vault_autolock_min", Value::from(10), Int),
        ("vault_trust_min", Value::from(30), Int),
        ("palette_hotkey", Value::from("Ctrl+Alt+K"), Str),
        ("clip_paste_hotkey", Value::from("Ctrl+Win+V"), Str),
        ("clip_quick_hotkey", Value::from("Ctrl+Alt+V"), Str),
        ("migrated_v2", Value::Bool(false), Bool),
        ("palette_commands", Value::from(""), Str),
    ]
}

fn default_map() -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v, _) in defaults() {
        m.insert(k.to_string(), v);
    }
    m
}

/// 类型收敛：值类型与基准类型不兼容时按 Python _typed 的规则收敛，
/// 无法收敛（语义损坏）则回退默认值。
fn coerce(key: &str, default: &Value, ty: Ty, v: &Value) -> Value {
    match ty {
        Ty::Bool => {
            if let Value::Bool(_) = v {
                return v.clone();
            }
            // 手改 0/1 → 按意图收敛
            if let Value::Number(n) = v {
                if let Some(i) = n.as_i64() {
                    if i == 0 || i == 1 {
                        return Value::Bool(i == 1);
                    }
                }
            }
        }
        Ty::Int => {
            match v {
                // true/false 写进 int 键
                Value::Bool(b) => return Value::from(i64::from(*b)),
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        return Value::from(i);
                    }
                    if let Some(f) = n.as_f64() {
                        if f.fract() == 0.0 && f.is_finite() {
                            return Value::from(f as i64);
                        }
                    }
                }
                // "50" → 50（保留手改意图）
                Value::String(s) => {
                    if let Ok(i) = s.trim().parse::<i64>() {
                        return Value::from(i);
                    }
                }
                _ => {}
            }
        }
        Ty::Str => {
            if let Value::String(_) = v {
                return v.clone();
            }
        }
    }
    log::warn!("settings: 键 {} 值类型异常，已回退默认 {default}", key);
    default.clone()
}

/// 对整份设置做类型收敛（只动 DEFAULTS 已知键，动态键原样保留）。
pub fn typed(data: &Map<String, Value>) -> Map<String, Value> {
    let mut out = data.clone();
    for (k, default, ty) in defaults() {
        if let Some(v) = data.get(k) {
            let fixed = coerce(k, &default, ty, v);
            out.insert(k.to_string(), fixed);
        }
    }
    out
}

fn cache() -> &'static Mutex<Option<Map<String, Value>>> {
    static CACHE: OnceLock<Mutex<Option<Map<String, Value>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 清空进程缓存（仅测试用：切换数据目录/手工改盘后重新读取）。
#[cfg(test)]
pub fn reset_cache() {
    *cache().lock().expect("settings 缓存锁中毒") = None;
}

/// 读盘（可指定路径，测试用）；损坏时回退默认值并留痕。
pub fn load_from(path: &Path) -> Map<String, Value> {
    let mut data = default_map();
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(stored)) => {
                for (k, v) in stored {
                    data.insert(k, v);
                }
            }
            Ok(_) => log::warn!("settings.json 顶层非对象，已忽略: {}", paths::display(path)),
            Err(e) => log::warn!(
                "settings.json 读取失败，已回退默认设置: {} ({e})",
                paths::display(path)
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!(
            "settings.json 读取失败，已回退默认设置: {} ({e})",
            paths::display(path)
        ),
    }
    typed(&data)
}

/// 读取设置（进程内缓存，与 Python 的 _cache 同语义）。
pub fn load() -> Map<String, Value> {
    let mut guard = cache().lock().expect("settings 缓存锁中毒");
    if let Some(c) = guard.as_ref() {
        return c.clone();
    }
    let data = load_from(&paths::settings_file());
    *guard = Some(data.clone());
    data
}

/// 单键取布尔（缺失回退默认）。
pub fn get_bool(key: &str) -> bool {
    let v = load().get(key).cloned().unwrap_or(Value::Bool(false));
    v.as_bool().unwrap_or(false)
}

/// 单键取整数（缺失/损坏回退默认值表，再回退 0）。
pub fn get_int(key: &str) -> i64 {
    let v = load().get(key).cloned().unwrap_or(Value::Null);
    if let Some(i) = v.as_i64() {
        return i;
    }
    defaults()
        .into_iter()
        .find(|(k, _, _)| *k == key)
        .and_then(|(_, d, _)| d.as_i64())
        .unwrap_or(0)
}

/// 单键取字符串（缺失回退默认值表，再回退空串）。
pub fn get_str(key: &str) -> String {
    let v = load().get(key).cloned().unwrap_or(Value::Null);
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    defaults()
        .into_iter()
        .find(|(k, _, _)| *k == key)
        .and_then(|(_, d, _)| d.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// 合并写入（补丁同样过类型收敛）；先更新缓存，写盘失败向上抛错。
pub fn save(patch: &Map<String, Value>) -> std::io::Result<()> {
    save_to(&paths::settings_file(), patch)
}

/// 指定路径写入（测试用；同时更新缓存，与 Python 先更新缓存同语义）。
pub fn save_to(path: &Path, patch: &Map<String, Value>) -> std::io::Result<()> {
    let mut merged = load();
    for (k, v) in patch {
        merged.insert(k.clone(), v.clone());
    }
    let merged = typed(&merged);
    *cache().lock().expect("settings 缓存锁中毒") = Some(merged.clone());

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(merged))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::stores::atomic_write(path, text.as_bytes())
}

/// 序列化后的键值对（BTreeMap 仅用于稳定比较，不作为写盘格式）。
pub fn as_sorted(data: &Map<String, Value>) -> BTreeMap<String, Value> {
    data.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_cover_python_keys() {
        let keys: Vec<&str> = defaults().into_iter().map(|(k, _, _)| k).collect();
        assert_eq!(keys.len(), 24, "DEFAULTS 键数应与 config.py 对齐");
        for must in [
            "clipboard_max",
            "clipboard_watch",
            "clip_paste_hotkey",
            "clip_quick_hotkey",
            "palette_hotkey",
            "clip_process_blacklist",
        ] {
            assert!(keys.contains(&must), "缺少键 {must}");
        }
    }

    #[test]
    fn coerce_int_from_string_and_float() {
        let d = json!(30);
        assert_eq!(
            coerce("poll_interval_sec", &d, Ty::Int, &json!("50")),
            json!(50)
        );
        assert_eq!(
            coerce("poll_interval_sec", &d, Ty::Int, &json!(60.0)),
            json!(60)
        );
        assert_eq!(
            coerce("poll_interval_sec", &d, Ty::Int, &json!(true)),
            json!(1)
        );
        // 无法收敛 → 回退默认
        assert_eq!(coerce("poll_interval_sec", &d, Ty::Int, &json!("很快")), d);
        assert_eq!(coerce("poll_interval_sec", &d, Ty::Int, &json!(1.5)), d);
    }

    #[test]
    fn coerce_bool_from_zero_one() {
        let d = json!(false);
        assert_eq!(coerce("dark_mode", &d, Ty::Bool, &json!(1)), json!(true));
        assert_eq!(coerce("dark_mode", &d, Ty::Bool, &json!(0)), json!(false));
        assert_eq!(coerce("dark_mode", &d, Ty::Bool, &json!("yes")), d);
    }

    #[test]
    fn coerce_str_rejects_numbers() {
        let d = json!("阿里");
        assert_eq!(
            coerce("dns_preset", &d, Ty::Str, &json!("腾讯")),
            json!("腾讯")
        );
        assert_eq!(coerce("dns_preset", &d, Ty::Str, &json!(5)), d);
        assert_eq!(coerce("dns_preset", &d, Ty::Str, &json!(null)), d);
    }

    #[test]
    fn typed_keeps_dynamic_keys() {
        let mut m = Map::new();
        m.insert("last_page".into(), json!(5));
        m.insert("poll_interval_sec".into(), json!("很快"));
        let out = typed(&m);
        assert_eq!(out.get("last_page"), Some(&json!(5)), "动态键必须原样保留");
        assert_eq!(out.get("poll_interval_sec"), Some(&json!(30)));
    }

    #[test]
    fn load_missing_file_returns_defaults() {
        let p = std::env::temp_dir().join("syskit_rs_nonexistent_settings.json");
        let _ = std::fs::remove_file(&p);
        let d = load_from(&p);
        assert_eq!(d.get("clipboard_max"), Some(&json!(50)));
        assert_eq!(d.get("clip_paste_hotkey"), Some(&json!("Ctrl+Win+V")));
    }

    #[test]
    fn load_corrupt_file_falls_back() {
        let dir = std::env::temp_dir().join("syskit_rs_settings_corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        std::fs::write(&p, "{ 半截").unwrap();
        let d = load_from(&p);
        assert_eq!(d.get("clipboard_max"), Some(&json!(50)));
    }
}
