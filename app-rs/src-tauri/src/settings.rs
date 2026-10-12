//! 设置持久化：与 Python 侧 settings.py 同语义（DEFAULTS + 类型收敛 + 原子写）。
//!
//! 为什么必须有类型收敛这层：文件半截 JSON 有兜底，但「合法 JSON + 错误类型」
//! （手改/被其他工具写坏，如 poll_interval_sec="很快"）会原样穿透到消费方。
//! 只校验 DEFAULTS 已知键：last_page / modes_prev_volume 等动态键原样保留。

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

/// 已知设置键 → (默认值, 类型)。逐键对齐 config.py::DEFAULTS（覆盖全部键，
/// 键数由下方 `defaults_cover_python_keys` 测试锁定，勿手改数字）。
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
        ("clipboard_max", Value::from(1000), Int),
        ("clipboard_watch", Value::Bool(false), Bool),
        ("clipboard_retain_days", Value::from(30), Int),
        (
            "clip_process_blacklist",
            Value::from("keepass,bitwarden,1password,enpass"),
            Str,
        ),
        ("clip_sensitive_ttl", Value::from(25), Int),
        ("clip_sensitive_detect", Value::Bool(true), Bool),
        ("quality_probe_enabled", Value::Bool(true), Bool),
        ("quality_alert_enabled", Value::Bool(false), Bool),
        ("quality_alert_latency_ms", Value::from(200), Int),
        ("quality_alert_loss_pct", Value::from(20), Int),
        ("traffic_alert_gb", Value::from(0), Int),
        ("clip_capture_images", Value::Bool(true), Bool),
        ("vault_autolock_min", Value::from(30), Int),
        ("vault_trust_min", Value::from(30), Int),
        ("palette_hotkey", Value::from("Ctrl+Alt+K"), Str),
        ("clip_paste_hotkey", Value::from("Ctrl+Win+V"), Str),
        ("clip_quick_hotkey", Value::from("Ctrl+Alt+V"), Str),
        ("migrated_v2", Value::Bool(false), Bool),
        ("palette_commands", Value::from(""), Str),
    ]
}

/// 一个设置项的界面元数据。
///
/// **`defaults()` 是键清单的唯一来源**，这里只补"给人看"的信息：
/// 缺元数据的键会退回 `key` 作为标签、并给空描述，**不会从界面上消失**。
/// 这样元数据不会成为"漏掉某个键"的第二处真相。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SettingMeta {
    pub key: String,
    pub label: String,
    pub description: String,
    /// `bool` / `int` / `str`
    pub ty: String,
    pub default: Value,
    pub current: Value,
    /// 整数项的合法范围（越界会被拒绝写入）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
    /// 字符串项的可选值（非空时界面用下拉框）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// 是否重启后生效（界面标注）
    pub needs_restart: bool,
    /// 非空表示**该设置目前没有消费方**（改了不生效），内容为原因说明。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub unwired_reason: String,
}

/// 界面装饰信息（标签 / 描述 / 范围 / 可选值 / 是否重启生效 / 未接线说明）。
///
/// 用结构体而不是六元组：六元组在调用处要靠位置记忆，极易把 `min`/`max` 写反，
/// 而写反的范围会让校验变成"总是拒绝"或"总是接受"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoration {
    pub label: &'static str,
    pub description: &'static str,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub options: Option<Vec<String>>,
    pub needs_restart: bool,
    /// 非空表示**该设置目前没有消费方**（改了不生效），文案说明原因。
    ///
    /// 为什么必须标出来：设置页给了开关却没人读，用户改了没效果只会以为"坏了"
    /// 或"我改错了"。诚实地写明"暂未生效"是唯一可接受的呈现。
    pub unwired_reason: &'static str,
}

impl Decoration {
    /// 没有元数据时的占位：标签留空，由调用方退回键名。
    fn none() -> Self {
        Self {
            label: "",
            description: "",
            min: None,
            max: None,
            options: None,
            needs_restart: false,
            unwired_reason: "",
        }
    }

    fn new(label: &'static str, description: &'static str) -> Self {
        Self {
            label,
            description,
            ..Self::none()
        }
    }

    fn restart(label: &'static str, description: &'static str) -> Self {
        Self {
            needs_restart: true,
            ..Self::new(label, description)
        }
    }

    fn int(label: &'static str, description: &'static str, min: i64, max: i64) -> Self {
        Self {
            min: Some(min),
            max: Some(max),
            ..Self::new(label, description)
        }
    }

    fn choice(label: &'static str, description: &'static str, opts: &[&str]) -> Self {
        Self {
            options: Some(opts.iter().map(|s| s.to_string()).collect()),
            ..Self::new(label, description)
        }
    }
}

/// 界面装饰信息。
fn decoration(key: &str) -> Decoration {
    match key {
        // ⚠ 以下六项已由 monitor.rs 接线：后台监控 + 保守自动修复梯度
        "auto_repair_enabled" => {
            Decoration::new("自动修复", "连通性持续失败时自动执行保守修复梯度")
        }
        "poll_interval_sec" => Decoration::int("检测间隔（秒）", "后台连通性探测的间隔", 10, 3600),
        "auto_repair_max_level" => Decoration::int(
            "自动修复最高等级",
            "L1=刷DNS L2=+起NlaSvc/续租 L3=+重启WLAN/断合网卡",
            1,
            3,
        ),
        "fail_threshold" => Decoration::int("连续失败次数阈值", "连续失败几次触发修复", 1, 20),
        "cooldown_sec" => Decoration::int("修复冷却（秒）", "两次修复之间的最小间隔", 0, 86400),
        "level3_cooldown_sec" => Decoration::int(
            "三级修复冷却（秒）",
            "L3 会短暂断网，此冷却内自动降级为 L2",
            0,
            86400,
        ),
        "minimize_to_tray" => Decoration::new("关闭时最小化到托盘", "点关闭按钮不退出程序"),
        "tray_rate_enabled" => {
            Decoration::new("托盘显示实时速率", "把上下行速率显示在托盘悬浮提示里")
        }
        "sound_alert_enabled" => Decoration::new("声音提醒", "断网/恢复时播放提示音"),
        "dark_mode" => Decoration::new("深色主题", "切换界面深浅配色"),
        "dns_preset" => Decoration::choice(
            "默认 DNS 预设",
            "修复动作 dns_preset 未指定时使用的预设",
            &["阿里", "腾讯", "114", "360"],
        ),
        "clipboard_max" => Decoration::int(
            "剪贴板历史上限",
            "最多保留多少条（置顶不参与淘汰）",
            1,
            1000,
        ),
        "clipboard_watch" => Decoration::restart("剪贴板监听", "总开关；关闭只停新记录，历史保留"),
        "clipboard_retain_days" => {
            Decoration::int("剪贴板保留天数", "0=永不过期；置顶条目不受影响", 0, 3650)
        }
        "clip_process_blacklist" => {
            Decoration::new("剪贴板黑名单", "前台是这些进程时不记录（逗号分隔）")
        }
        "clip_sensitive_ttl" => Decoration::int(
            "敏感内容存活（秒）",
            "复制敏感内容后自动清空剪贴板的秒数；0=不清空（仍不进历史与系统剪贴板历史）",
            0,
            3600,
        ),
        "clip_capture_images" => Decoration::new("记录剪贴板图片", "关闭后只记录文本与文件路径"),
        "clip_sensitive_detect" => Decoration::new(
            "敏感内容不入库",
            "复制的内容像信用卡号/身份证号/API 密钥时不记入剪贴板历史",
        ),
        "quality_probe_enabled" => Decoration::new(
            "网络质量探测",
            "周期探测网关/公网延迟与丢包，驱动「网络质量」趋势卡",
        ),
        "quality_alert_enabled" => Decoration::new(
            "网络质量告警",
            "公网延迟/丢包持续超过阈值时托盘通知并记入事件时间线；恢复时也通知（10 分钟冷却防轰炸）",
        ),
        "quality_alert_latency_ms" => Decoration::int(
            "告警延迟阈值（ms）",
            "最近 1 分钟公网平均延迟超过该值触发告警",
            50,
            5000,
        ),
        "quality_alert_loss_pct" => Decoration::int(
            "告警丢包阈值（%）",
            "最近 1 分钟公网丢包率超过该值触发告警",
            1,
            100,
        ),
        "traffic_alert_gb" => Decoration::int(
            "流量超额提醒（GB/天）",
            "今日收发合计达到限额 80% 与 100% 时各托盘提醒一次；0=关闭",
            0,
            1024,
        ),
        "vault_autolock_min" => Decoration::int(
            "密码库自动锁定（分）",
            "闲置（无密码库操作）超过该时长后，下一次操作会要求重新解锁；0=不自动锁定",
            0,
            1440,
        ),
        "vault_trust_min" => Decoration::int(
            "主密码免重复输入（分）",
            "验证通过后该窗口内不再要求重新输入主密码；0=每次都要求",
            0,
            1440,
        ),
        "palette_hotkey" => Decoration::new("命令面板热键", "保存后即时生效；被占用会自动回退旧值"),
        "clip_paste_hotkey" => Decoration::new(
            "快捷粘贴面板热键",
            "保存后即时生效；被占用自动回退 Ctrl+Alt+V 预设，仍失败则禁用；空=禁用",
        ),
        "clip_quick_hotkey" => Decoration::new(
            "快速粘贴热键",
            "保存后即时生效，不弹面板直接粘贴最新一条；被占用自动回退旧值",
        ),
        // migrated_v2 是内部迁移标志，本就无需消费方；不标「暂未生效」以免误导
        "migrated_v2" => Decoration::new("双热键迁移标志", "内部标志，一次性迁移用；一般无需改动"),
        "palette_commands" => {
            Decoration::new("命令面板自定义命令", "JSON 数组 [{name,type,value}]")
        }
        // defaults() 里新增而这里还没补元数据时走这一支：标签退回键名，不从界面消失
        _ => Decoration::none(),
    }
}

/// 全部设置的当前值与界面元数据。
pub fn describe_all() -> Vec<SettingMeta> {
    let cur = load();
    defaults()
        .into_iter()
        .map(|(key, default, ty)| {
            let ty_str = match ty {
                Ty::Bool => "bool",
                Ty::Int => "int",
                Ty::Str => "str",
            };
            let current = cur.get(key).cloned().unwrap_or_else(|| default.clone());
            let d = decoration(key);
            SettingMeta {
                key: key.to_string(),
                label: if d.label.is_empty() {
                    key.to_string()
                } else {
                    d.label.to_string()
                },
                description: d.description.to_string(),
                ty: ty_str.to_string(),
                default,
                current,
                min: d.min,
                max: d.max,
                options: d.options,
                needs_restart: d.needs_restart,
                unwired_reason: d.unwired_reason.to_string(),
            }
        })
        .collect()
}

/// 写入一个设置项（按键的类型收敛 + 范围校验）。
///
/// 范围校验放在这里而不是只靠界面：界面可以被绕过，而 `poll_interval_sec=0`
/// 会让后台线程忙碌空转、`clipboard_max=0` 会让剪贴板功能直接失效。
pub fn set_one(key: &str, value: Value) -> Result<Value, String> {
    let Some((_, default, ty)) = defaults().into_iter().find(|(k, _, _)| *k == key) else {
        return Err(format!("未知设置项: {key}"));
    };
    // 先做类型收敛（与读路径同一套规则），再校验范围
    let coerced = coerce(key, &default, ty, &value);
    if matches!(coerced, Value::Null) {
        return Err(format!("{key} 的值类型不合法"));
    }
    let d = decoration(key);
    let (min, max, options) = (d.min, d.max, d.options);
    if let Some(n) = coerced.as_i64() {
        if let Some(lo) = min {
            if n < lo {
                return Err(format!("{key} 不能小于 {lo}（当前填 {n}）"));
            }
        }
        if let Some(hi) = max {
            if n > hi {
                return Err(format!("{key} 不能大于 {hi}（当前填 {n}）"));
            }
        }
    }
    if let (Some(opts), Some(s)) = (&options, coerced.as_str()) {
        if !opts.iter().any(|o| o == s) {
            return Err(format!("{key} 只能是 {} 之一", opts.join(" / ")));
        }
    }
    let mut patch = Map::new();
    patch.insert(key.to_string(), coerced.clone());
    save(&patch).map_err(|e| format!("写入设置失败: {e}"))?;
    Ok(coerced)
}

/// 恢复默认值（只重置已知键，动态键如 `modes_prev_volume` 保留）。
pub fn reset_to_defaults() -> Result<usize, String> {
    let patch = default_map();
    let n = patch.len();
    save(&patch).map_err(|e| format!("写入设置失败: {e}"))?;
    Ok(n)
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

/// 动态键取整数：**不做默认值回退**，缺失就是 None。
///
/// 与 `get_int` 的区别在于用途：`get_int` 服务已知设置键（有默认值表）；
/// 这个用于 `modes_prev_volume` 这类**动态键**，调用方需要区分"没有记录"与
/// "记录是 0"——前者要设为最大音量，后者要重新存档。
pub fn get_i64(key: &str) -> Option<i64> {
    load().get(key).and_then(|v| v.as_i64())
}

/// 动态键写整数（合并写入，其它键不动）。
pub fn set_i64(key: &str, value: i64) -> std::io::Result<()> {
    let mut m = Map::new();
    m.insert(key.to_string(), Value::from(value));
    save(&m)
}

/// 当前设置的完整快照（已按 DEFAULTS 收敛类型）。
///
/// 场景模式的"备份配置"在没有历史文件时用它落一份非空备份——空备份目录对用户
/// 毫无意义，还会让"备份成功"的提示变成误导。
pub fn snapshot_json() -> Map<String, Value> {
    load()
}

/// 合并写入（补丁同样过类型收敛）；先更新缓存，写盘失败向上抛错。
pub fn save(patch: &Map<String, Value>) -> std::io::Result<()> {
    save_to(&paths::settings_file(), patch)
}

/// 删除动态键（`save` 的逆操作；场景模式旧单键 `modes_active` 迁移清场用）。
///
/// 缓存与磁盘同步更新；一个键都不存在时**不动盘**（幂等迁移的快速路径）。
pub fn remove_keys(keys: &[&str]) -> std::io::Result<()> {
    let mut merged = load();
    let mut removed_any = false;
    for k in keys {
        removed_any |= merged.remove(*k).is_some();
    }
    if !removed_any {
        return Ok(());
    }
    *cache().lock().expect("settings 缓存锁中毒") = Some(merged.clone());
    crate::stores::save_json_pretty(&paths::settings_file(), &Value::Object(merged))
}

/// 指定路径写入（测试用；同时更新缓存，与 Python 先更新缓存同语义）。
pub fn save_to(path: &Path, patch: &Map<String, Value>) -> std::io::Result<()> {
    let mut merged = load();
    for (k, v) in patch {
        merged.insert(k.clone(), v.clone());
    }
    let merged = typed(&merged);
    *cache().lock().expect("settings 缓存锁中毒") = Some(merged.clone());
    crate::stores::save_json_pretty(path, &Value::Object(merged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---------------- 界面元数据（M7 设置页） ----------------

    #[test]
    fn describe_all_covers_every_default_key() {
        // 元数据是装饰层，**绝不能成为"漏掉某个键"的第二处真相**：
        // 键数必须与 defaults() 完全一致
        let described = describe_all();
        assert_eq!(
            described.len(),
            defaults().len(),
            "describe_all 必须覆盖全部默认键"
        );
        let keys: Vec<String> = described.iter().map(|m| m.key.clone()).collect();
        for (k, _, _) in defaults() {
            assert!(keys.contains(&k.to_string()), "{k} 没有出现在设置页里");
        }
    }

    #[test]
    fn every_key_has_a_human_label() {
        // 缺元数据时退回键名（不是空串），保证界面不会出现空白标签
        for m in describe_all() {
            assert!(!m.label.is_empty(), "{} 的标签为空", m.key);
            assert!(!m.ty.is_empty(), "{} 的类型为空", m.key);
        }
    }

    #[test]
    fn decoration_covers_all_keys_so_none_falls_back_to_raw_name() {
        // 这条是"元数据补全度"的看门检查：新增键却忘了写标签会在这里失败，
        // 而不是等用户看到一串英文键名
        for (k, _, _) in defaults() {
            let d = decoration(k);
            assert!(!d.label.is_empty(), "{k} 缺少界面标签");
            assert!(!d.description.is_empty(), "{k} 缺少说明文字");
        }
    }

    #[test]
    fn int_keys_declare_a_range() {
        // 整数项必须给范围：poll_interval_sec=0 会让后台线程空转，
        // clipboard_max=0 会让剪贴板功能失效——界面要能拦住
        for m in describe_all() {
            if m.ty == "int" {
                assert!(
                    m.min.is_some() && m.max.is_some(),
                    "{} 是整数项但没有声明范围",
                    m.key
                );
                let lo = m.min.unwrap();
                let hi = m.max.unwrap();
                assert!(lo <= hi, "{} 的范围颠倒", m.key);
                let d = m.default.as_i64().unwrap();
                assert!(
                    d >= lo && d <= hi,
                    "{} 的默认值 {d} 不在声明的范围 [{lo},{hi}] 内",
                    m.key
                );
            }
        }
    }

    #[test]
    fn dns_preset_options_match_repairs_constants() {
        let m = describe_all()
            .into_iter()
            .find(|m| m.key == "dns_preset")
            .unwrap();
        let opts = m.options.expect("dns_preset 应有下拉选项");
        assert_eq!(opts, vec!["阿里", "腾讯", "114", "360"]);
        // 与修复动作里的预设必须一致，否则界面能选但动作不认
        for o in opts {
            assert!(
                crate::repairs::dns_preset(&o).is_some(),
                "{o} 在设置页可选，但 repairs::dns_preset 不认识它"
            );
        }
    }

    #[test]
    fn describe_reports_current_not_default() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        set_one("poll_interval_sec", json!(120)).unwrap();
        let m = describe_all()
            .into_iter()
            .find(|m| m.key == "poll_interval_sec")
            .unwrap();
        assert_eq!(m.current, json!(120), "应报告当前值");
        assert_eq!(m.default, json!(30), "默认值仍是 30");
    }

    // ---------------- set_one 的校验 ----------------

    #[test]
    fn set_one_rejects_unknown_key() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let e = set_one("no_such_key", json!(1)).unwrap_err();
        assert!(e.contains("未知设置项"), "{e}");
    }

    #[test]
    fn set_one_enforces_range() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        // poll_interval_sec 范围 10..3600：0 会让后台线程空转
        let e = set_one("poll_interval_sec", json!(0)).unwrap_err();
        assert!(e.contains("不能小于"), "{e}");
        let e2 = set_one("poll_interval_sec", json!(999999)).unwrap_err();
        assert!(e2.contains("不能大于"), "{e2}");
        // 边界值本身要接受
        assert_eq!(set_one("poll_interval_sec", json!(10)).unwrap(), json!(10));
        assert_eq!(
            set_one("poll_interval_sec", json!(3600)).unwrap(),
            json!(3600)
        );
    }

    #[test]
    fn set_one_accepts_zero_where_zero_is_meaningful() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        // 0 在语义上是"不限制"的项不该被范围拦掉
        assert_eq!(
            set_one("clipboard_retain_days", json!(0)).unwrap(),
            json!(0)
        );
        assert_eq!(set_one("vault_autolock_min", json!(0)).unwrap(), json!(0));
        assert_eq!(set_one("clip_sensitive_ttl", json!(0)).unwrap(), json!(0));
    }

    #[test]
    fn set_one_validates_enum_options() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        assert_eq!(set_one("dns_preset", json!("腾讯")).unwrap(), json!("腾讯"));
        let e = set_one("dns_preset", json!("不存在的预设")).unwrap_err();
        assert!(e.contains("只能是"), "{e}");
    }

    #[test]
    fn set_one_coerces_types_like_python() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        // 手改 0/1 按意图收敛成布尔（与读路径同一套规则）
        assert_eq!(set_one("dark_mode", json!(1)).unwrap(), json!(true));
        assert_eq!(set_one("dark_mode", json!(0)).unwrap(), json!(false));
        // 数字字符串收敛成整数
        assert_eq!(set_one("fail_threshold", json!("5")).unwrap(), json!(5));
        // 布尔写进整数键 → 0/1
        assert_eq!(set_one("fail_threshold", json!(true)).unwrap(), json!(1));
    }

    #[test]
    fn set_one_falls_back_to_default_for_uncoercible_bool() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        // 字符串 "true" **不**被收敛（Python 的 _typed 同样只认 0/1 数字），
        // 而是回退默认值——这是既有语义，不是 bug；这里锁住它免得后人"顺手改成
        // 认字符串"从而与 Python 版行为分叉。
        set_one("dark_mode", json!(true)).unwrap();
        assert_eq!(set_one("dark_mode", json!("true")).unwrap(), json!(false));
        // 同理，无法收敛的整数也回退默认而不是报错
        assert_eq!(
            set_one("fail_threshold", json!("很快")).unwrap(),
            json!(2),
            "无法收敛时回退默认值"
        );
    }

    #[test]
    fn reset_to_defaults_restores_every_key_and_keeps_dynamic_ones() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        set_one("poll_interval_sec", json!(120)).unwrap();
        set_one("dark_mode", json!(true)).unwrap();
        // 动态键（不在 defaults 里）不该被重置掉
        set_i64("modes_prev_volume", 12345).unwrap();

        let n = reset_to_defaults().unwrap();
        assert_eq!(n, defaults().len());
        let cur = load();
        assert_eq!(cur.get("poll_interval_sec"), Some(&json!(30)));
        assert_eq!(cur.get("dark_mode"), Some(&json!(false)));
        assert_eq!(
            cur.get("modes_prev_volume"),
            Some(&json!(12345)),
            "动态键必须保留（否则场景模式的音量记录会被重置清掉）"
        );
    }

    #[test]
    fn describe_serializes_with_expected_fields() {
        let j = serde_json::to_string(&describe_all()[0]).unwrap();
        for k in ["key", "label", "ty", "default", "current", "needs_restart"] {
            assert!(j.contains(k), "序列化应含 {k}: {j}");
        }
    }

    // ---------------- 「暂未生效」标注（诚实性） ----------------

    /// 审计结论：这些键当前**没有消费方**，改了不生效，必须在界面上说明。
    ///
    /// 这个清单是**手工审计的产物**（逐键搜 `src-tauri/src/` 与 `src/` 里的引用）。
    /// 它同时是一份待办：接上消费方后应把该键从清单里删掉，删掉时这条测试会失败
    /// ——这正是我们想要的提醒（否则容易忘记摘掉过期的"暂未生效"标签）。
    /// vault_autolock_min/vault_trust_min 已随 M6 数据层接线（会话自动重锁/
    /// trust 窗口），摘除。
    /// 全部接线完成，名单归零（minimize_to_tray=lib.rs 关窗事件；
    /// sound_alert_enabled=monitor 发声门控；vault_* 两项随 M6 接线）。新键
    /// 若暂无消费方，加回这里并在 Decoration 上写「暂未生效：…」。
    const UNWIRED: [&str; 0] = [];

    #[test]
    fn unwired_keys_are_marked_and_explained() {
        let items = describe_all();
        for k in UNWIRED {
            let m = items
                .iter()
                .find(|m| m.key == k)
                .unwrap_or_else(|| panic!("{k} 不在 defaults() 里"));
            assert!(
                !m.unwired_reason.is_empty(),
                "{k} 没有消费方，必须标注「暂未生效」并说明原因"
            );
            assert!(
                m.unwired_reason.contains("暂未生效"),
                "{k} 的说明必须以「暂未生效」开头，便于界面统一识别：{}",
                m.unwired_reason
            );
        }
    }

    #[test]
    fn wired_keys_are_not_marked_unwired() {
        // 反向：完全接线的项不得带任何提示，否则用户会不敢用。
        for m in describe_all() {
            if UNWIRED.contains(&m.key.as_str()) {
                continue;
            }
            assert!(
                m.unwired_reason.is_empty(),
                "{} 已接线，不应标任何「暂未生效」提示：{}",
                m.key,
                m.unwired_reason
            );
        }
    }

    #[test]
    fn clip_capture_images_is_now_fully_wired() {
        // 图片捕获本轮补全（CF_DIB 解析 + PNG 编码），不再是"部分生效"。
        // 这条用例的存在意义：标注必须随实现更新——过期标签会变成新的谎言。
        let m = describe_all()
            .into_iter()
            .find(|m| m.key == "clip_capture_images")
            .unwrap();
        assert!(
            m.unwired_reason.is_empty(),
            "图片捕获已完整实现，不该再标「暂未生效」或「部分生效」：{}",
            m.unwired_reason
        );
    }

    #[test]
    fn hotkey_keys_are_instant_effect() {
        // 三热键已实现「保存后即时重挂+被占回退」（Step 2），不得再标需重启——
        // 标注随实现更新：过期标签会变成新的谎言。
        for key in ["palette_hotkey", "clip_quick_hotkey", "clip_paste_hotkey"] {
            let m = describe_all()
                .into_iter()
                .find(|m| m.key == key)
                .unwrap_or_else(|| panic!("{key} 不在 defaults() 里"));
            assert!(!m.needs_restart, "{key} 已即时重挂，不该再标「需重启」");
        }
    }

    #[test]
    fn internal_flag_is_not_marked_unwired() {
        // migrated_v2 是内部标志，本就无需消费方；标「暂未生效」会误导
        let m = describe_all()
            .into_iter()
            .find(|m| m.key == "migrated_v2")
            .unwrap();
        assert!(m.unwired_reason.is_empty());
    }

    #[test]
    fn defaults_cover_python_keys() {
        let keys: Vec<&str> = defaults().into_iter().map(|(k, _, _)| k).collect();
        assert_eq!(
            keys.len(),
            30,
            "DEFAULTS 键数（24 Python 对齐 + clip_sensitive_detect + quality_probe_enabled
            + quality_alert_enabled/latency_ms/loss_pct + traffic_alert_gb）"
        );
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
        assert_eq!(d.get("clipboard_max"), Some(&json!(1000)));
        assert_eq!(d.get("clip_paste_hotkey"), Some(&json!("Ctrl+Win+V")));
    }

    #[test]
    fn load_corrupt_file_falls_back() {
        let dir = std::env::temp_dir().join("syskit_rs_settings_corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        std::fs::write(&p, "{ 半截").unwrap();
        let d = load_from(&p);
        assert_eq!(d.get("clipboard_max"), Some(&json!(1000)));
    }
}
