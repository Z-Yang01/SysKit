//! Tauri 前端可调命令（`invoke` 接口）。所有命令都是薄封装，业务逻辑在各模块里。

use crate::clipboard_history::ClipItem;
use crate::settings;

// ---------------- 按域拆分的子模块（v1.3 架构优化，渐进迁移） ----------------
// re-export 保持 lib.rs 的 commands::sk_xxx 引用不变；尚未拆出的域
// （诊断修复/工具箱杂项/剪贴板/设置日志/系统助手/命令面板）仍在本文件。
mod network;
pub use network::*;
mod toolbox;
pub use toolbox::*;
mod diag;
pub use diag::*;
mod clipboard;
mod vault;
pub use clipboard::*;
mod palette;
pub use palette::*;
pub use vault::*;
pub use vault::{VaultSession, VaultTrust};

/// 面板/主窗口共用的条目行（含原始下标，供写回定位）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipRow {
    pub idx: usize,
    pub ts: i64,
    pub text: String,
    pub kind: String,
    pub pinned: bool,
    pub data: String,
    /// 单行预览（列表展示用）。
    pub preview: String,
}

impl ClipRow {
    fn from_item(it: &ClipItem) -> Self {
        Self {
            idx: it.idx.unwrap_or(0),
            ts: it.ts,
            text: it.text.clone(),
            kind: it.kind.clone(),
            pinned: it.pinned,
            data: it.data.clone(),
            preview: ClipItem::preview(&it.text, 80),
        }
    }
}

/// 探活（M0 通路验证保留）。
#[tauri::command]
pub fn ping() -> String {
    "pong".to_string()
}

/// 读取剪贴板历史（可按关键字/类型过滤）。
#[tauri::command]
pub fn sk_setting_get(key: String) -> serde_json::Value {
    let data = settings::load();
    data.get(&key).cloned().unwrap_or(serde_json::Value::Null)
}

/// 主窗口标题栏用的版本信息。
#[tauri::command]
pub fn sk_version() -> serde_json::Value {
    serde_json::json!({
        "version": crate::paths::VERSION,
        "display_name": crate::paths::DISPLAY_NAME,
    })
}

// ---------------- M7 设置 / 日志 ----------------

/// 全部设置的当前值与界面元数据。
#[tauri::command]
pub fn sk_settings_all() -> serde_json::Value {
    serde_json::json!({
        "items": crate::settings::describe_all(),
        "count": crate::settings::defaults().len(),
    })
}

/// 写入一个设置项（类型收敛 + 范围校验都在后端做，界面可被绕过）。
///
/// 热键三键保存后即时重挂（语义对齐 Python `_rehook_hotkey`）：被占回退
/// 旧值并持久化 + 发 `sk://hotkey-reverted`；`previous` 快照在写入前捕获。
/// 保存成功一律广播 `sk://settings-changed`：主窗的 dark 状态等「设置驱动的
/// 运行时状态」以此同步（此前深色开关只写文件不改 UI 状态，勾选不变暗）。
#[tauri::command]
pub fn sk_settings_set(
    key: String,
    value: serde_json::Value,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    use tauri::Emitter;
    const HOTKEY_KEYS: [&str; 3] = ["palette_hotkey", "clip_quick_hotkey", "clip_paste_hotkey"];
    let previous = if HOTKEY_KEYS.contains(&key.as_str()) {
        Some(crate::settings::get_str(&key))
    } else {
        None
    };
    let saved = crate::settings::set_one(&key, value)?;
    let _ = app.emit(
        "sk://settings-changed",
        serde_json::json!({ "key": key, "value": saved }),
    );
    if let Some(previous) = previous {
        let prev_getter = |k: &str| -> String {
            if k == key {
                previous.clone()
            } else {
                crate::settings::get_str(k)
            }
        };
        crate::apply_hotkeys(&app, &prev_getter);
    }
    Ok(serde_json::json!({ "key": key, "value": saved, "message": "已保存" }))
}

/// 热键三键的重置回退目标快照（纯读，供 `sk_settings_reset` 与单测）。
fn hotkey_reset_fallbacks() -> [String; 3] {
    ["palette_hotkey", "clip_quick_hotkey", "clip_paste_hotkey"].map(crate::settings::get_str)
}

/// 恢复默认设置的公共体（`rehook` 注入以便单测）：**先**快照热键三键旧值，
/// **再**落默认，最后把快照交给 `rehook` 即时重挂。顺序是语义的一部分——
/// 落了默认再快照就只剩默认值，被占回退会因「回退目标==请求值」失去回退步，
/// 用户旧组合从此找不回（轮次37 变异验证锁定此顺序）。
fn reset_settings_with_rehook<R>(rehook: R) -> Result<String, String>
where
    R: FnOnce(&dyn Fn(&str) -> String),
{
    let previous = hotkey_reset_fallbacks();
    let n = crate::settings::reset_to_defaults()?;
    let prev_getter = move |k: &str| -> String {
        match k {
            "palette_hotkey" => previous[0].clone(),
            "clip_quick_hotkey" => previous[1].clone(),
            "clip_paste_hotkey" => previous[2].clone(),
            _ => String::new(),
        }
    };
    rehook(&prev_getter);
    Ok(format!(
        "已恢复 {n} 项默认值（动态键如 modes_prev_volume 保留）"
    ))
}

/// 恢复默认值（只重置已知键，动态键保留）。
///
/// 热键三键重置后**必须即时重挂**（轮次37 互审实锤）：只写设置不重挂，
/// 注册链还挂着用户旧组合，设置页却显示默认组合——按默认组合按了没反应，
/// 三方一致（注册链 × 设置页 × 侧栏速查）被 reset 路径击穿。重置**前**快照
/// 旧值作为回退目标：默认组合被占时链回退到旧值并持久化 + 发
/// `sk://hotkey-reverted`，与 `sk_settings_set` 保存路径同语义。
#[tauri::command]
pub fn sk_settings_reset(app: tauri::AppHandle) -> Result<String, String> {
    reset_settings_with_rehook(|prev| {
        crate::apply_hotkeys(&app, prev);
    })
}

/// 日志文件列表（按修改时间倒序）。
#[tauri::command]
pub fn sk_log_list() -> serde_json::Value {
    let dir = crate::paths::log_dir();
    let mut files: Vec<serde_json::Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let meta = e.metadata().ok();
            let name = e.file_name().to_string_lossy().to_string();
            // 只列日志类文件，避免把目录里的杂项也展示出来
            if !name.ends_with(".log") && !name.ends_with(".txt") {
                continue;
            }
            files.push(serde_json::json!({
                "name": name,
                "bytes": meta.as_ref().map(|m| m.len()).unwrap_or(0),
                "modified": meta
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            }));
        }
    }
    files.sort_by(|a, b| b["modified"].as_u64().cmp(&a["modified"].as_u64()));
    serde_json::json!({
        "dir": dir.to_string_lossy(),
        "files": files,
        "count": files.len(),
    })
}

/// 读取日志尾部若干行（**只读尾部**：日志可能很大，整读会卡住界面）。
#[tauri::command]
pub async fn sk_log_tail(name: String, lines: Option<usize>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // 防目录穿越：只允许纯文件名
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(format!("非法的日志文件名: {name}"));
        }
        let path = crate::paths::log_dir().join(&name);
        if !path.is_file() {
            return Err(format!("日志文件不存在: {name}"));
        }
        let n = lines.unwrap_or(500).clamp(1, 5000);
        let bytes = std::fs::read(&path).map_err(|e| format!("读取失败: {e}"))?;
        // 日志既有 UTF-8 也有 ANSI 的可能：lossy 解码保证不因个别字节整篇失败
        let text = String::from_utf8_lossy(&bytes);
        let all: Vec<&str> = text.lines().collect();
        let start = all.len().saturating_sub(n);
        let tail: Vec<&str> = all[start..].to_vec();
        Ok(serde_json::json!({
            "name": name,
            "total_lines": all.len(),
            "shown": tail.len(),
            "truncated": start > 0,
            "lines": tail,
        }))
    })
    .await
    .map_err(|e| format!("读取线程失败: {e}"))?
}

// ---------------- M5 系统助手：场景模式 ----------------

/// 场景模式清单（供前端渲染按钮与说明）。
#[tauri::command]
pub fn sk_modes_info() -> serde_json::Value {
    migrate_modes_active_keys();
    serde_json::json!({
        "common_apps": crate::modes::COMMON_APPS
            .iter()
            .map(|(n, t)| serde_json::json!({ "name": n, "target": t }))
            .collect::<Vec<_>>(),
        "office_default_apps": crate::modes::OFFICE_DEFAULT_APPS,
        "whitelist": crate::modes::BACKGROUND_CLOSE_WHITELIST,
        "is_admin": crate::w32::is_elevated(),
        // 当前生效模式（settings 双键 `modes_active_office`/`modes_active_game`，
        // 旧单键已就地迁移；run 全部步骤成功才置位，退出即清自己族）
        "active_mode": {
            "office": modes_flag_on("modes_active_office"),
            "game": modes_flag_on("modes_active_game"),
        },
        "notes": {
            "office": "勿扰（关通知）+ 静音 + 打开常用软件 + 备份当前配置。退出模式会恢复通知并取消静音。",
            "game": "关通知 + 高性能电源 + 暂停 Windows 更新 + 清理白名单后台。退出模式会恢复通知、平衡电源与更新策略。",
            "update_policy": "「暂停更新」通过 NoAutoUpdate 策略实现，是持久生效的（不是暂停 N 天），需退出游戏模式或手动恢复。",
            "admin": "暂停更新与部分电源操作需要管理员权限；失败步骤会如实列出，不影响其它步骤。"
        }
    })
}

/// 场景模式双键取值：开 = "1"。旧单键存的是模式名（"office"/"game"/""），
/// 双键拆族后每键只有 开/关 两态，"1"/"0" 与 `get_str`（缺失→空串→关）直配。
fn modes_flag_str(on: bool) -> &'static str {
    if on {
        "1"
    } else {
        "0"
    }
}

/// 场景模式双键读取（缺失/空串/脏值一律视为关）。
fn modes_flag_on(key: &str) -> bool {
    crate::settings::get_str(key) == "1"
}

/// 旧单键 `modes_active` 的值 → 双键初值 `(office, game)`（纯函数，供单测）。
pub fn modes_dual_from_legacy(legacy: &str) -> (bool, bool) {
    (legacy == "office", legacy == "game")
}

/// 一次性迁移：读到旧单键 `modes_active` 时按其值初始化双键并**清旧键**。
///
/// 幂等：旧键不存在直接返回（不动盘）；旧键显式为 ""（旧版退出写过）也走
/// 迁移——双键落 "0" 后旧键删除，此后读写面只剩双键。save 成功但清键失败时
/// 旧键仍在，下次读到会再迁一遍（写同值 + 再清），收敛。
fn migrate_modes_active_keys() {
    let Some(v) = crate::settings::load().get("modes_active").cloned() else {
        return;
    };
    let (office, game) = modes_dual_from_legacy(v.as_str().unwrap_or(""));
    let mut patch = serde_json::Map::new();
    patch.insert(
        "modes_active_office".to_string(),
        serde_json::Value::String(modes_flag_str(office).to_string()),
    );
    patch.insert(
        "modes_active_game".to_string(),
        serde_json::Value::String(modes_flag_str(game).to_string()),
    );
    if let Err(e) = crate::settings::save(&patch) {
        log::warn!("场景模式双键迁移写入失败（旧键保留，下次再迁）: {e}");
        return;
    }
    if let Err(e) = crate::settings::remove_keys(&["modes_active"]) {
        log::warn!("场景模式旧单键清除失败（下次再迁）: {e}");
    }
}

/// 场景模式执行后双键（`modes_active_office` / `modes_active_game`）的目标
/// 状态（纯函数，供单测）。`cur` = 双键当前值 `(office, game)`。
///
/// 返回 `Some((office, game))` = 双键目标；`None` = 部分失败/未知模式，系统
/// 真实状态未知，**不动旧值**——沿用轮次32 `modes_active_after_run` 的「失败
/// 不动」语义（更早的旧实现失败写空串：既抹掉仍在生效的另一模式标记，也在
/// 退出失败时谎报已还原）。
///
/// 互斥在**写入侧**强制：进入任一模式成功即清另一族（同刻只应一个场景模式
/// 生效；旧单键天然如此，双键拆族后由这里保持，显示侧只读）。退出只动自己
/// 那族——另一族标记是否真实生效未知，不能替它「顺手清」（旧前端实现任意
/// 退出清双标记，会丢仍在生效那族的「进行中」）。
pub fn modes_dual_after_run(
    mode: &str,
    fail_count: usize,
    cur: (bool, bool),
) -> Option<(bool, bool)> {
    match (mode, fail_count) {
        ("office", 0) => Some((true, false)),
        ("game", 0) => Some((false, true)),
        ("office_exit", 0) => Some((false, cur.1)),
        ("game_exit", 0) => Some((cur.0, false)),
        _ => None,
    }
}

/// 进入模式前的**互斥前置还原**判定（纯函数，供单测/变异验证）。
///
/// 标记互斥的另一面是系统态也必须跟着还：进入 office 成功会把 game 标记清成
/// "未激活"，但 `run_office_mode` 本身**不还原**游戏族的改动（NoAutoUpdate
/// 策略、高性能电源）——不补前置还原，标记说"游戏已退出"而系统仍带着游戏
/// 模式的持久策略（标记与真实系统态分叉，且 UI 的"进行中"提示从此消失，
/// 用户失去了"需要退出游戏模式来恢复更新"的线索）。
///
/// 返回 Some(还原族) = 进入前先真实执行该族的 restore；None = 无需。
pub fn mutual_entry_restore(mode: &str, cur: (bool, bool)) -> Option<&'static str> {
    match (mode, cur) {
        ("office", (_, true)) => Some("game_exit"),
        ("game", (true, _)) => Some("office_exit"),
        _ => None,
    }
}

/// 一次 run 的**执行计划**（纯函数，供单测/变异验证）：按真实执行顺序返回
/// 模式步 id 列表。另一族标记在生效时，前置还原步必须排在进入步**之前**——
/// 两族第 1 步都是「通知/勿扰」且写同一个注册表值：先进入再还原，restore 的
/// 「开启通知」会把刚进入模式的勿扰覆盖掉（轮次37 互审实锤：勿扰静默失效，
/// 结果清单的步序也和执行序相反）。未知模式返回空表（调用方报错，绝不执行
/// 任何系统动作）。
pub fn modes_run_plan(mode: &str, cur: (bool, bool)) -> Vec<&'static str> {
    if !matches!(mode, "office" | "office_exit" | "game" | "game_exit") {
        return Vec::new();
    }
    match mutual_entry_restore(mode, cur) {
        // 前置还原步在前，进入步在后（步序 = 执行序 = 清单序）
        Some(restore_id) => vec![restore_id, if mode == "office" { "office" } else { "game" }],
        None => vec![match mode {
            "office" => "office",
            "office_exit" => "office_exit",
            "game" => "game",
            _ => "game_exit",
        }],
    }
}

/// 进入/退出场景模式（每步结果一并返回，单步失败不中断）。执行顺序由
/// `modes_run_plan` 给出：另一族标记在生效时**先真实退出该族再进入**——
/// 两族第 1 步都是「通知/勿扰」且写同一个注册表值，先进入再还原会让
/// restore 的「开启通知」覆盖刚进入模式的勿扰（轮次37 互审实锤）。restore
/// 步如实并入结果清单（步序=执行序）；前置还原失败 → fail_count>0 → 双键
/// 不动旧值，与「失败不写标记」的既有语义衔接。
#[tauri::command]
pub async fn sk_modes_run(
    mode: String,
    open_apps: Option<bool>,
    clear_bg: Option<bool>,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        migrate_modes_active_keys();
        let cur = (
            modes_flag_on("modes_active_office"),
            modes_flag_on("modes_active_game"),
        );
        let plan = modes_run_plan(&mode, cur);
        if plan.is_empty() {
            return Err(format!("未知模式: {mode}"));
        }
        let mut mode_name = String::new();
        let mut steps = Vec::new();
        for id in plan {
            let out = match id {
                "office" => crate::modes::run_office_mode(open_apps.unwrap_or(true)),
                "office_exit" => crate::modes::restore_office_defaults(),
                "game" => crate::modes::run_game_mode(clear_bg.unwrap_or(true)),
                "game_exit" => crate::modes::restore_game_defaults(),
                other => return Err(format!("未知还原族: {other}")),
            };
            if id == mode.as_str() {
                mode_name = out.mode.clone();
            } else {
                log::info!(
                    "互斥前置还原：进入 {} 前先执行 {}（另一族标记在生效）",
                    mode,
                    id
                );
            }
            steps.extend(out.steps);
        }
        let outcome = crate::modes::ModeOutcome::new(&mode_name, steps);
        let summary = outcome.summary();
        // 当前模式持久化：跨会话可见（settings 双键）。只在**结局确定**时
        // 写：进入成功→置自己族并清另一族（互斥在写入侧强制），退出成功→
        // 清自己族（另一族保持）；部分失败时系统真实状态未知，**不动旧值**
        // （前端 applyModeRun 同语义：失败保持原标记）。旧单键 `modes_active`
        // 只能存一个模式名：office 生效中进 game（哪怕部分失败没写成）也会在
        // 下一次成功写入时丢掉 office 标记，且旧前端任意退出清双标记——双键
        // 后两族各自存退，丢态序列不再存在。
        if let Some((office, game)) = modes_dual_after_run(
            &mode,
            outcome.fail_count,
            (
                modes_flag_on("modes_active_office"),
                modes_flag_on("modes_active_game"),
            ),
        ) {
            let mut patch = serde_json::Map::new();
            patch.insert(
                "modes_active_office".to_string(),
                serde_json::Value::String(modes_flag_str(office).to_string()),
            );
            patch.insert(
                "modes_active_game".to_string(),
                serde_json::Value::String(modes_flag_str(game).to_string()),
            );
            if let Err(e) = crate::settings::save(&patch) {
                log::warn!("模式状态持久化失败（不影响本次执行）: {e}");
            }
        }
        Ok(serde_json::json!({
            "outcome": outcome,
            "summary": summary,
        }))
    })
    .await
    .map_err(|e| format!("模式线程失败: {e}"))?
}

/// 列出可清理的后台候选（白名单内正在运行的）。
#[tauri::command]
pub async fn sk_modes_bg_candidates() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let list: Vec<serde_json::Value> = crate::modes::background_candidates()
            .into_iter()
            .map(|(pid, name)| serde_json::json!({ "pid": pid, "name": name }))
            .collect();
        Ok(serde_json::json!({ "rows": list, "count": list.len() }))
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

// ---------------- M5 系统助手：电池 ----------------

/// 实时电源状态（`GetSystemPowerStatus`，微秒级，可频繁调用）。
#[tauri::command]
pub fn sk_power_status() -> serde_json::Value {
    let p = crate::battery::read_power();
    serde_json::json!({
        "ac_online": p.ac_online,
        "has_battery": p.has_battery,
        "percent": p.percent,
        "charging": p.charging,
        "seconds_left": p.seconds_left,
        "remaining_human": crate::battery::human_remaining(p.seconds_left),
        "summary": crate::battery::power_summary(&p),
    })
}

/// 电池健康度（WMI，**最坏 30 秒**）——只由用户点刷新触发，不做轮询。
#[tauri::command]
pub async fn sk_battery_health() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let h = crate::battery::query_health();
        Ok(serde_json::json!({
            "design_mwh": h.design_mwh,
            "full_mwh": h.full_mwh,
            "health_percent": h.health_percent,
            "token": crate::battery::health_token(h.health_percent),
            "available": h.is_available(),
            "note": crate::battery::health_note(&h),
        }))
    })
    .await
    .map_err(|e| format!("查询线程失败: {e}"))?
}

// ---------------- M5 系统助手：磁盘空间 / 垃圾清理 ----------------

/// 目录空间分析：递归扫描目录树（大小/文件数/类型分布/大文件 Top200）。
/// 路径校验与 sk_disk_scan_children 完全同源（diskusage::validate_scan_dir，
/// 轮次114 补齐：绝对路径/存在且为目录/拒绝设备命名空间与 DOS 保留设备名）。
/// 取消走 sk_disk_scan_cancel（同一时刻只跑一个扫描）。
#[tauri::command]
pub async fn sk_disk_scan_tree(root: String) -> Result<serde_json::Value, String> {
    let root = crate::diskusage::validate_scan_dir(&root)?;
    crate::diskusage::SCAN_STOP.reset();
    tauri::async_runtime::spawn_blocking(move || {
        let r = crate::diskusage::scan_tree(&root, &crate::diskusage::SCAN_STOP);
        serde_json::to_value(r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

/// 取消进行中的目录扫描（已入回收站的删除不受影响）。
#[tauri::command]
pub fn sk_disk_scan_cancel() {
    crate::diskusage::SCAN_STOP.cancel();
}

/// 目录逐级展开：扫描一个目录的直接子项（递归大小/类型分布/Top200 摘要，
/// 只返回一层——孙辈由前端再次调用按需下钻）。路径校验见
/// diskusage::validate_scan_dir（绝对路径/存在且为目录/拒绝设备命名空间与
/// DOS 保留设备名）。取消走 sk_disk_scan_cancel（与 sk_disk_scan_tree
/// 同一枚 SCAN_STOP，同一时刻只跑一个扫描）。
#[tauri::command]
pub async fn sk_disk_scan_children(dir: String) -> Result<serde_json::Value, String> {
    let dir = crate::diskusage::validate_scan_dir(&dir)?;
    crate::diskusage::SCAN_STOP.reset();
    tauri::async_runtime::spawn_blocking(move || {
        let r = crate::diskusage::scan_children(&dir, &crate::diskusage::SCAN_STOP);
        serde_json::to_value(r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

/// 批量移入回收站（可还原），返回 (成功数, 失败路径列表)。
#[tauri::command]
pub async fn sk_disk_recycle_delete(paths: Vec<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (ok, errors) = crate::diskusage::delete_to_recycle_bin(&paths);
        serde_json::json!({ "ok_count": ok, "errors": errors })
    })
    .await
    .map_err(|e| format!("删除线程失败: {e}"))
}

/// 列出本地卷的容量与使用率。
#[tauri::command]
pub async fn sk_disk_list() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let drives = crate::disk::sort_by_pressure(crate::disk::list_drives());
        let rows: Vec<serde_json::Value> = drives
            .iter()
            .map(|d| {
                serde_json::json!({
                    "letter": d.letter,
                    "label": d.label,
                    "fs": d.fs,
                    "total": d.total,
                    "used": d.used(),
                    "free_total": d.free_total,
                    "free_to_caller": d.free_to_caller,
                    "used_percent": d.used_percent(),
                    "removable": d.removable,
                    "token": crate::disk::usage_token(d.used_percent()),
                })
            })
            .collect();
        Ok(serde_json::json!({ "rows": rows, "count": rows.len() }))
    })
    .await
    .map_err(|e| format!("枚举线程失败: {e}"))?
}

/// 清理目标清单（供前端渲染勾选项）。
#[tauri::command]
pub fn sk_clean_targets() -> serde_json::Value {
    serde_json::json!({
        "targets": crate::cleanup::CLEAN_TARGETS
            .iter()
            .map(|(t, title)| serde_json::json!({
                "key": t.key(),
                "title": title,
                "roots": t.roots().iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>()
    })
}

/// 扫描各清理目标的大小（**只读，不删任何东西**）。
///
/// 真机 TEMP 目录可能有很多文件，这一步会慢（秒级），所以走阻塞线程池。
#[tauri::command]
pub async fn sk_clean_scan() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut total = 0u64;
        let rows: Vec<serde_json::Value> = crate::cleanup::CLEAN_TARGETS
            .iter()
            .map(|(t, title)| {
                let bytes = crate::cleanup::scan_target(*t);
                total += bytes;
                serde_json::json!({
                    "key": t.key(),
                    "title": title,
                    "bytes": bytes,
                    "human": crate::disk::human_size(bytes),
                    "roots": t.roots().iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(serde_json::json!({
            "rows": rows,
            "total_bytes": total,
            "total_human": crate::disk::human_size(total),
        }))
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

/// 清理选中的目标（只删文件，跳过正在使用的）。
#[tauri::command]
pub async fn sk_clean_run(keys: Vec<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if keys.is_empty() {
            return Err("请先选择要清理的项目".to_string());
        }
        let mut total = crate::cleanup::CleanOutcome::empty();
        let mut per_target = Vec::new();
        for key in &keys {
            let Some(t) = crate::cleanup::CleanTarget::from_key(key) else {
                continue; // 未知 key 直接忽略，不做任何猜测
            };
            match t {
                crate::cleanup::CleanTarget::Recycle => {
                    // 回收站走 Shell COM（没有可遍历的目录路径）
                    let out = crate::w32::run_capture(
                        &[
                            "powershell".into(),
                            "-NoProfile".into(),
                            "-NonInteractive".into(),
                            "-Command".into(),
                            "Clear-RecycleBin -Force -ErrorAction SilentlyContinue".into(),
                        ],
                        60,
                    );
                    let ok = out.as_ref().map(|o| o.ok).unwrap_or(false);
                    per_target.push(serde_json::json!({
                        "key": t.key(),
                        "title": t.title(),
                        "ok": ok,
                        "message": if ok {
                            "回收站已清空（无法统计释放大小）"
                        } else {
                            "清空回收站失败或本来就是空的"
                        },
                    }));
                }
                _ => {
                    let out = crate::cleanup::clean_target(t);
                    per_target.push(serde_json::json!({
                        "key": t.key(),
                        "title": t.title(),
                        "ok": true,
                        "deleted": out.deleted,
                        "freed": out.freed,
                        "failed": out.failed,
                        "message": out.summary(),
                    }));
                    total = total.merge(out);
                }
            }
        }
        Ok(serde_json::json!({
            "outcome": total,
            "summary": total.summary(),
            "per_target": per_target,
        }))
    })
    .await
    .map_err(|e| format!("清理线程失败: {e}"))?
}

// ---------------- M5 系统助手：启动项管理 ----------------

/// 枚举启动项（HKCU/HKLM 的 Run 键 + 启动文件夹 + SysKit 的禁用记录）。
#[tauri::command]
pub async fn sk_startup_list() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let items = crate::startup::enumerate_real();
        let (on, off) = crate::startup::startup_stats(&items);
        Ok(serde_json::json!({
            "rows": items,
            "count": items.len(),
            "enabled": on,
            "disabled": off,
            "is_admin": crate::w32::is_elevated(),
        }))
    })
    .await
    .map_err(|e| format!("枚举线程失败: {e}"))?
}

/// 禁用/启用一个启动项。
///
/// 注册表项是**搬**到 `HKCU\Software\NetTool\DisabledRun`（沿用 Python 版的键名），
/// 启动文件夹项是改名加 `.disabled`。两版共享同一套标记，迁移期互不打架。
#[tauri::command]
pub async fn sk_startup_set(
    location: String,
    name: String,
    enable: bool,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::startup::set_enabled_real(&location, &name, enable)?;
        Ok(if enable {
            format!("已启用 {name}")
        } else {
            format!("已禁用 {name}（可从本页重新启用）")
        })
    })
    .await
    .map_err(|e| format!("执行线程失败: {e}"))?
}

// ---------------- M5 系统助手：Top 进程 ----------------

/// Top 进程缓存（tasklist 一次约 0.5 秒，2 秒窗口内复用同一份）。
static TOP_PROCS_CACHE: std::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>> =
    std::sync::Mutex::new(None);

/// 内存占用最高的 N 个进程。
///
/// 数据源用 `tasklist /fo csv`（约 0.5 秒）而不是 `Get-CimInstance
/// Win32_PerfFormattedData_PerfProc_Process`（实测 3.3 秒，且 Name 是
/// `chrome#20` 这种性能计数器实例名而非真实进程名），也不是 `Get-Process`
/// （0.3 秒但要多一个解析分支）。选 tasklist 的决定性理由：**端口页已经在解析
/// 同一份 CSV**，复用同一份规则就没有第二处解析可能出错。
#[tauri::command]
pub async fn sk_top_procs(limit: Option<usize>) -> Result<serde_json::Value, String> {
    let n = limit.unwrap_or(15).clamp(1, 100);
    tauri::async_runtime::spawn_blocking(move || {
        const TTL_MS: u128 = 2000;
        {
            let g = TOP_PROCS_CACHE.lock().expect("进程缓存锁中毒");
            if let Some((at, v)) = g.as_ref() {
                if at.elapsed().as_millis() < TTL_MS {
                    return Ok(v.clone());
                }
            }
        }
        let out = crate::w32::run_capture(&args(&["tasklist", "/fo", "csv", "/nh"]), 30)
            .map_err(|e| format!("tasklist 无法执行: {e}"))?;
        let procs = crate::portscan::parse_tasklist_mem(&out.stdout);
        if procs.is_empty() {
            return Err("未能解析出任何进程（tasklist 输出格式可能有变）".to_string());
        }
        let (count, total) = crate::portscan::procs_summary(&procs);
        let top: Vec<serde_json::Value> = crate::portscan::top_by_mem(procs, n)
            .into_iter()
            .map(|p| {
                serde_json::json!({
                    "pid": p.pid,
                    "name": p.name,
                    "bytes": p.bytes,
                })
            })
            .collect();
        let value = serde_json::json!({
            "rows": top,
            "count": count,
            "total_bytes": total,
            "limit": n,
        });
        *TOP_PROCS_CACHE.lock().expect("进程缓存锁中毒") =
            Some((std::time::Instant::now(), value.clone()));
        Ok(value)
    })
    .await
    .map_err(|e| format!("采样线程失败: {e}"))?
}

// ---------------- M5 系统助手：WiFi 状态 ----------------

/// 读取 WiFi 状态（`wlanapi.dll`，微秒级）。
#[tauri::command]
pub fn sk_wifi_info() -> serde_json::Value {
    let w = crate::wifi::read();
    serde_json::json!({
        "connected": w.connected,
        "ssid": w.ssid,
        "signal": w.signal,
        "dbm": w.dbm,
        "bssid": w.bssid,
        "rx_kbps": w.rx_kbps,
        "tx_kbps": w.tx_kbps,
        "label": w.signal.map(crate::wifi::signal_label).unwrap_or("未连接"),
    })
}

/// Wake-on-LAN：向指定 MAC 发魔术包（UDP 广播，默认端口 9）。
///
/// `direct_broadcast` 可选：网段定向广播地址（如 `192.168.1.255`），与全局
/// 广播各发一次提高命中率；MAC 解析失败（位数/非法字符）返回 Err 由前端提示。
#[tauri::command]
pub async fn sk_wol_send(
    mac: String,
    port: Option<u16>,
    direct_broadcast: Option<String>,
) -> Result<String, String> {
    let parsed = crate::wol::parse_mac(&mac)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::wol::send_wol(&parsed, port.unwrap_or(9), direct_broadcast.as_deref())
    })
    .await
    .map_err(|e| format!("WOL 线程失败: {e}"))?
}

// ---------------- M4 工具箱：局域网设备 ----------------

/// 扫描本机所在 /24 网段：并发 ping 唤醒邻居 → 读 ARP 表列出存活设备。
#[tauri::command]
pub async fn sk_lan_scan(prefix: Option<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let prefix = match prefix.filter(|p| !p.trim().is_empty()) {
            Some(p) => crate::portscan::normalize_prefix(p.trim()),
            None => {
                // 用快照自洽的物理网卡（单一 PowerShell 数据源，IP 归并天然一致）。
                // 此前跨源 join（GetIfTable2 别名 ↔ 快照按名相等）在本机对不上，
                // 有 IPv4 也报"未找到"——真机功能扫实锤 2026-10-04。
                let snap = crate::snapshot::take();
                let ip = snap
                    .physical_up_adapters()
                    .first()
                    .and_then(|a| a.ipv4.first().cloned())
                    .ok_or_else(|| {
                        // 带上采集自身错误：快照失败（PowerShell 异常/解析失败）
                        // 与"真的没有物理网卡"是两回事，混在一起没法排障
                        format!(
                            "未找到可用的本机 IPv4 地址，无法推断网段（快照：{}）",
                            if snap.error.is_empty() {
                                "无错误，快照正常"
                            } else {
                                &snap.error
                            }
                        )
                    })?;
                crate::portscan::subnet24_of(&ip)
                    .ok_or_else(|| format!("无法从 {ip} 推断 /24 网段"))?
            }
        };
        let hosts = crate::portscan::hosts_in_subnet24(&prefix);
        // 并发 ping：256 个线程太重，用 32 路分批（每批 ~1 秒，总 ~8 秒）
        const BATCH: usize = 32;
        for chunk in hosts.chunks(BATCH) {
            let handles: Vec<_> = chunk
                .iter()
                .map(|h| {
                    let h = h.clone();
                    std::thread::spawn(move || {
                        let _ = crate::w32::run_capture(
                            &args(&["ping", "-n", "1", "-w", "500", &h]),
                            3,
                        );
                    })
                })
                .collect();
            for h in handles {
                let _ = h.join();
            }
        }
        let arp = crate::w32::run_capture(&args(&["arp", "-a"]), 20)
            .map_err(|e| format!("arp 无法执行: {e}"))?;
        let mut devices: Vec<serde_json::Value> = crate::portscan::parse_arp(&arp.stdout)
            .into_iter()
            .filter(crate::portscan::is_useful_neighbor)
            .filter(|e| e.ip.starts_with(&format!("{prefix}.")))
            .map(|e| {
                // 再 ping 一次拿延迟（ARP 表已暖，这次很快）
                let latency =
                    crate::w32::run_capture(&args(&["ping", "-n", "1", "-w", "800", &e.ip]), 3)
                        .ok()
                        .filter(|o| o.ok)
                        .and_then(|o| crate::portscan::parse_ping_ms(&o.combined()));
                serde_json::json!({
                    "ip": e.ip,
                    "mac": e.mac,
                    "kind": e.kind,
                    "latency_ms": latency,
                })
            })
            .collect();
        devices.sort_by_key(|d| {
            d["ip"]
                .as_str()
                .and_then(|s| s.rsplit('.').next())
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0)
        });
        Ok(serde_json::json!({
            "prefix": prefix,
            "scanned": hosts.len(),
            "devices": devices,
            "count": devices.len(),
        }))
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use vault::{mask_password, session_lock, vault_autolock_due};

    #[test]
    fn mask_password_rules() {
        // 长度 ≥6：前2 + **** + 后2
        assert_eq!(mask_password("Str0ng!Pass"), "St****ss");
        // 中文按字符计（不是字节）
        assert_eq!(mask_password("密码测试一二"), "密码****一二");
        // 长度 <6：一律 ****（防掩码等于原文）
        assert_eq!(mask_password("abc"), "****");
        assert_eq!(mask_password("abcde"), "****");
        assert_eq!(mask_password(""), "****");
    }

    #[test]
    fn modes_dual_keys_persist_only_on_decisive_outcomes() {
        // 进入成功 → 置自己族 + 清另一族（互斥在写入侧强制：同刻只应一个场景
        // 模式生效；旧单键天然如此，双键拆族后由纯函数保持）
        assert_eq!(
            modes_dual_after_run("office", 0, (false, true)),
            Some((true, false)),
            "进办公清游戏标记"
        );
        assert_eq!(
            modes_dual_after_run("game", 0, (true, false)),
            Some((false, true)),
            "进游戏清办公标记"
        );
        assert_eq!(
            modes_dual_after_run("office", 0, (false, false)),
            Some((true, false))
        );
        assert_eq!(
            modes_dual_after_run("game", 0, (false, false)),
            Some((false, true))
        );
        // 退出成功 → 只清自己那族，另一族保持（不能替它「顺手清」）
        assert_eq!(
            modes_dual_after_run("office_exit", 0, (true, false)),
            Some((false, false))
        );
        assert_eq!(
            modes_dual_after_run("game_exit", 0, (false, true)),
            Some((false, false))
        );
        assert_eq!(
            modes_dual_after_run("office_exit", 0, (true, true)),
            Some((false, true)),
            "退出办公不清游戏标记"
        );
        assert_eq!(
            modes_dual_after_run("game_exit", 0, (true, true)),
            Some((true, false)),
            "退出游戏不清办公标记"
        );
        // 部分失败：真实状态未知 → None（不动旧值）。回归锁：旧实现写空串，
        // 会抹掉仍在生效的另一模式标记（office→game 失败分叉），且退出失败
        // 时 NoAutoUpdate=1 仍生效却被清，重启后「进行中」与真实状态相反。
        assert_eq!(modes_dual_after_run("office", 1, (true, true)), None);
        assert_eq!(modes_dual_after_run("game", 3, (true, true)), None);
        assert_eq!(modes_dual_after_run("office_exit", 1, (true, false)), None);
        assert_eq!(modes_dual_after_run("game_exit", 1, (true, false)), None);
        // 未知模式一律 None（settings 不得被写进垃圾值）
        assert_eq!(modes_dual_after_run("bogus", 0, (false, false)), None);
    }

    #[test]
    fn vault_autolock_due_boundary_matches_setting() {
        // 边界含等号（与 monitor 冷却同风格）：恰满窗口即锁
        assert!(vault_autolock_due(30.0 * 60.0, 30));
        assert!(!vault_autolock_due(30.0 * 60.0 - 0.5, 30));
        // 0 与负值 = 用户显式关闭自动锁，闲置再久也不锁
        assert!(!vault_autolock_due(1e9, 0));
        assert!(!vault_autolock_due(1e9, -5));
    }

    #[test]
    fn vault_session_autolock_is_idle_based_not_since_unlock() {
        // 回归锁（无人值守轮次36）：自动重锁必须是**闲置**超时（对齐 Python
        // _ensure_unlocked → _restart_autolock）。按"解锁那一刻"起算的旧语义下，
        // 连续操作的用户也会被中途踢出，TOTP 轮询显示反复被「密码库未解锁」打断
        // ——正在进行的批量取密/复制与锁定定时交错（跨模块审计场景6）。
        let _g = crate::test_support::serial();
        let _dir = crate::test_support::isolate();
        crate::settings::set_one("vault_autolock_min", serde_json::json!(30)).unwrap();
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0)
        };
        let unlocked_session = |tag: &str, idle_secs: f64| {
            let mut c = crate::vault_crypto::VaultCrypto::with_path(
                crate::paths::appdata_dir().join(format!("{tag}.key")),
            );
            c.setup(Some("pw")).unwrap();
            assert!(c.unlock(Some("pw")).unwrap());
            c.set_unlocked_at_for_test(now() - idle_secs);
            VaultSession(std::sync::Mutex::new(Some(c)))
        };

        // 闲置超窗（31 分钟 > 30 分钟）→ 下一次密码库操作前就地收锁
        let session = unlocked_session("autolock-over", 31.0 * 60.0);
        let err = match session_lock(&session) {
            Err(e) => e,
            Ok(_) => panic!("闲置超窗的会话必须被拒绝"),
        };
        assert!(err.contains("未解锁"), "{err}");
        assert!(
            session.0.lock().unwrap().is_none(),
            "超窗会话必须已收锁（密钥不得驻留内存）"
        );

        // 窗口内（29 分钟）→ 放行，且**放行即续期**：闲置计时归到本次操作
        let session = unlocked_session("autolock-in", 29.0 * 60.0);
        let _guard = session_lock(&session).unwrap();
        drop(_guard);
        let since = session
            .0
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|c| c.unlocked_since_secs())
            .expect("放行后会话必须仍在");
        assert!(
            now() - since < 60.0,
            "放行后闲置计时必须已归零（旧实现保持解锁时刻，续期无从谈起）：偏移 {}s",
            now() - since
        );

        // 0 = 不自动锁：闲置数小时也不得收锁
        crate::settings::set_one("vault_autolock_min", serde_json::json!(0)).unwrap();
        let session = unlocked_session("autolock-off", 3600.0 * 5.0);
        let _guard = session_lock(&session).expect("关闭自动锁后不得因闲置收锁");
        drop(_guard);
        assert!(session.0.lock().unwrap().is_some());
    }

    #[test]
    fn modes_legacy_single_key_maps_to_dual_initials() {
        assert_eq!(modes_dual_from_legacy("office"), (true, false));
        assert_eq!(modes_dual_from_legacy("game"), (false, true));
        // 旧版退出写过空串：迁移为双关，不得臆造激活态
        assert_eq!(modes_dual_from_legacy(""), (false, false));
        // 脏值（手改 settings.json）不进双键
        assert_eq!(modes_dual_from_legacy("garbage"), (false, false));
    }

    #[test]
    fn mutual_entry_restore_requires_real_other_family_exit() {
        // 回归锁（无人值守轮次36）：标记互斥的另一面是系统态。旧实现进入
        // office 直接清掉 game 标记，但 run_office_mode 不还原游戏族的
        // NoAutoUpdate/高性能电源——标记说"游戏已退出"而持久策略仍在系统里
        // 生效（标记与真实系统态分叉）。修复：进入前先真实还原另一族。
        // 游戏标记在生效 → 进 office 前必须先退出游戏族
        assert_eq!(
            mutual_entry_restore("office", (false, true)),
            Some("game_exit")
        );
        // 办公标记在生效 → 进 game 前必须先退出办公族（静音同样不会自愈）
        assert_eq!(
            mutual_entry_restore("game", (true, false)),
            Some("office_exit")
        );
        // 另一族未激活：直接进，不画蛇添足
        assert_eq!(mutual_entry_restore("office", (false, false)), None);
        assert_eq!(mutual_entry_restore("game", (false, false)), None);
        // 退出路径永不触发前置还原（退出只动自己族，既有语义）
        assert_eq!(mutual_entry_restore("office_exit", (false, true)), None);
        assert_eq!(mutual_entry_restore("game_exit", (true, false)), None);
        // 未知模式不触发
        assert_eq!(mutual_entry_restore("bogus", (true, true)), None);
        // 与 modes_dual_after_run 的写入语义闭环：进入成功 → 自己族置位、
        // 另一族清零；前置还原已在进入动作里真实发生过，标记才配清
        let cur = (false, true);
        if mutual_entry_restore("office", cur).is_some() {
            assert_eq!(
                modes_dual_after_run("office", 0, cur),
                Some((true, false)),
                "前置还原成功后进 office：game 标记清零才与系统态一致"
            );
        }
    }

    #[test]
    fn modes_run_plan_restores_other_family_before_entry() {
        // 回归锁（无人值守轮次37）：前置还原必须排在进入**之前**。两族第 1 步
        // 都是「通知/勿扰」且写同一注册表值（NOC_GLOBAL_SETTING_TOASTS_ENABLED）
        // ——先进入再还原，restore 的「开启通知」会覆盖刚进入模式的勿扰，
        // 且结果清单步序与执行序相反（轮次37 互审实锤的旧 r36 实现行为）。
        assert_eq!(
            modes_run_plan("office", (false, true)),
            vec!["game_exit", "office"]
        );
        assert_eq!(
            modes_run_plan("game", (true, false)),
            vec!["office_exit", "game"]
        );
        // 另一族未激活：只进不还
        assert_eq!(modes_run_plan("office", (false, false)), vec!["office"]);
        assert_eq!(modes_run_plan("game", (false, false)), vec!["game"]);
        // 退出路径永不触发前置还原（既有语义）
        assert_eq!(
            modes_run_plan("office_exit", (false, true)),
            vec!["office_exit"]
        );
        assert_eq!(
            modes_run_plan("game_exit", (true, false)),
            vec!["game_exit"]
        );
        // 未知模式：空计划（调用方报错，绝不执行任何系统动作）
        assert!(modes_run_plan("bogus", (true, true)).is_empty());
        assert!(modes_run_plan("", (false, false)).is_empty());
    }

    #[test]
    fn settings_reset_captures_hotkey_fallbacks_before_defaults() {
        // 回归锁（无人值守轮次37）：reset 的热键回退目标必须在落默认**前**快照
        // ——落了默认再读就只剩默认值，被占回退会因「回退目标==请求值」失去
        // 回退步，用户旧组合从此找不回。走真实公共体（rehook 用测试替身），
        // 快照/落默认的顺序变动必须在此变红。
        let _g = crate::test_support::serial();
        let _dir = crate::test_support::isolate();
        crate::settings::set_one("palette_hotkey", serde_json::json!("Ctrl+Alt+Z")).unwrap();
        let mut given_to_rehook: Option<String> = None;
        let summary = reset_settings_with_rehook(|prev| {
            given_to_rehook = Some(prev("palette_hotkey"));
        })
        .unwrap();
        assert_eq!(
            given_to_rehook.as_deref(),
            Some("Ctrl+Alt+Z"),
            "重挂钩拿到的回退目标必须是用户旧值（快照在落默认之前）"
        );
        assert!(summary.contains("已恢复"), "{summary}");
        // 落默认确实发生了：设置里已是默认组合
        assert_ne!(
            crate::settings::get_str("palette_hotkey"),
            "Ctrl+Alt+Z",
            "reset 必须把设置落回默认，重挂钩的旧值只作回退目标"
        );
    }

    #[test]
    fn modes_legacy_key_migrates_to_dual_and_is_cleared() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        // 预置旧单键（旧版存过 "game"）
        let mut legacy = serde_json::Map::new();
        legacy.insert(
            "modes_active".to_string(),
            serde_json::Value::String("game".to_string()),
        );
        crate::settings::save(&legacy).unwrap();

        migrate_modes_active_keys();

        assert_eq!(crate::settings::get_str("modes_active_office"), "0");
        assert_eq!(crate::settings::get_str("modes_active_game"), "1");
        assert_eq!(
            crate::settings::load().get("modes_active"),
            None,
            "旧单键必须被删除（否则每次读都重复迁移，且旧值可能被误读）"
        );

        // 幂等：旧键已无时迁移为无操作，不炸也不覆写既有双键
        let mut on = serde_json::Map::new();
        on.insert(
            "modes_active_office".to_string(),
            serde_json::Value::String("1".to_string()),
        );
        crate::settings::save(&on).unwrap();
        migrate_modes_active_keys();
        assert_eq!(crate::settings::get_str("modes_active_office"), "1");
        assert_eq!(crate::settings::get_str("modes_active_game"), "1");

        // 显式空串（旧版退出写过）也走迁移：双键落 0 + 清旧键
        let mut empty = serde_json::Map::new();
        empty.insert(
            "modes_active".to_string(),
            serde_json::Value::String(String::new()),
        );
        crate::settings::save(&empty).unwrap();
        migrate_modes_active_keys();
        assert_eq!(
            crate::settings::get_str("modes_active_office"),
            "0",
            "旧空串迁移为双关，不得臆造激活态"
        );
        assert_eq!(crate::settings::get_str("modes_active_game"), "0");
        assert_eq!(crate::settings::load().get("modes_active"), None);
    }

    #[test]
    fn source_has_no_raw_nul_bytes() {
        // sk_diag_export 的对话框过滤器曾把 \0 写成**裸 NUL 字节**：运行时
        // 语义不变（lpstrFilter 本就是 NUL 分隔），但源文件被 file/grep/编辑
        // 工具判成二进制——审计搜索静默漏配。同类调用方（图片/文本另存为）
        // 都用 \0 转义，此处锁死源码卫生。
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/mod.rs");
        let src = std::fs::read(&p).expect("读 commands/mod.rs");
        assert!(
            !src.contains(&0u8),
            "源码不得含裸 NUL 字节（应写成 \0\0 转义）"
        );
    }

    #[test]
    fn every_tauri_command_is_registered_in_lib() {
        // 接线守卫：commands/ 目录（mod.rs+各域文件）里声明的每个 #[tauri::command] 都必须出现在
        // lib.rs 的 generate_handler![...] 清单里。漏注册在编译期不报错、
        // 单测全绿，只在运行时 invoke 失败——sk_vault_copy_secret /
        // sk_clip_copy_plain 曾经就这样静默失联（前端按钮全废）。
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands");
        let cmds_src = std::fs::read_to_string(src_dir.join("mod.rs")).expect("读 commands/mod.rs");
        // 域文件（network/toolbox/vault/…）同样参与声明扫描：拆分后命令分散
        // 在子模块里，只读 mod.rs 会漏掉搬走的命令（漏注册运行时才炸）。
        let mut cmds_src_all = cmds_src;
        for domain in [
            "network.rs",
            "toolbox.rs",
            "vault.rs",
            "clipboard.rs",
            "palette.rs",
        ] {
            cmds_src_all.push_str(
                &std::fs::read_to_string(src_dir.join(domain))
                    .unwrap_or_else(|e| panic!("读 commands/{domain}: {e}")),
            );
        }
        let lib_src = std::fs::read_to_string(src_dir.join("../lib.rs")).expect("读 lib.rs");

        let declared: Vec<String> = cmds_src_all
            .split("#[tauri::command]")
            .skip(1)
            .filter_map(|chunk| {
                let idx = chunk.find("fn ")?;
                let rest = &chunk[idx + 3..];
                let name: String = rest
                    .chars()
                    .skip_while(|c| c.is_whitespace())
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if name.is_empty() {
                    None
                } else {
                    Some(name)
                }
            })
            .collect();
        assert!(
            declared.len() >= 100,
            "命令声明解析异常（只解析到 {} 个），守卫自身失效",
            declared.len()
        );

        let handler_zone = lib_src
            .split("generate_handler!")
            .nth(1)
            .expect("lib.rs 应有 generate_handler! 清单");
        let zone = &handler_zone[..handler_zone.find(']').expect("generate_handler 清单未闭合")];
        let registered: std::collections::HashSet<String> = zone
            .split("commands::")
            .skip(1)
            .map(|seg| {
                seg.chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect()
            })
            .collect();

        let missing: Vec<&str> = declared
            .iter()
            .map(String::as_str)
            .filter(|d| !registered.contains(*d))
            .collect();
        assert!(
            missing.is_empty(),
            "以下命令未在 lib.rs generate_handler 注册（运行时 invoke 必失败）: {missing:?}"
        );
    }

    #[test]
    fn clip_row_preview_and_index() {
        let long =
            "a very long clipboard entry that should be truncated in the list view".repeat(2);
        assert!(long.chars().count() > 80, "样本必须长于预览上限");
        let it = ClipItem {
            ts: 1700000000,
            text: long,
            kind: "text".into(),
            pinned: true,
            data: String::new(),
            dhash: None,
            idx: Some(3),
        };
        let row = ClipRow::from_item(&it);
        assert_eq!(row.idx, 3);
        assert!(row.pinned);
        assert!(row.preview.ends_with('…'), "超长必须截断: {}", row.preview);
        assert_eq!(row.preview.chars().count(), 80);
    }

    #[test]
    fn clip_row_without_idx_defaults_zero() {
        let it = ClipItem {
            ts: 1,
            text: "x".into(),
            kind: "text".into(),
            pinned: false,
            data: String::new(),
            dhash: None,
            idx: None,
        };
        assert_eq!(ClipRow::from_item(&it).idx, 0);
    }

    #[test]
    fn frontend_error_lands_in_syskit_log() {
        // ErrorBoundary 的上报端：错误必须进文件日志（release 下 devtools 不可见，
        // 文件日志是前端崩溃的唯一可诊断出口）
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        crate::logging::init();
        sk_frontend_error("boundary-marker-上抛测试".into());
        let dir = crate::paths::log_dir();
        let syskit = std::fs::read_to_string(dir.join("syskit.log")).unwrap_or_default();
        assert!(
            syskit.contains("[frontend] boundary-marker-上抛测试"),
            "前端错误未落 syskit.log"
        );
    }

    #[test]
    fn version_payload_matches_config() {
        let v = sk_version();
        assert_eq!(v["version"], crate::paths::VERSION);
        assert_eq!(v["display_name"], crate::paths::DISPLAY_NAME);
    }

    #[test]
    fn qr_svg_renders_local_and_scalable() {
        // url_qr 动作的渲染端：本地 SVG、内容可被浏览器 <img> 直接消费。
        // 纠错级别 M 与 Python qrcode.make 默认一致——这里锁住"能出图"与
        // "SVG 结构完整"两条底线，具体点阵形状交给 qrcode 库自己保证。
        let svg = sk_clip_qr_svg("https://example.com/rust".into()).expect("常规 URL 必须可渲染");
        assert!(
            svg.starts_with("<?xml") || svg.starts_with("<svg"),
            "必须是 SVG 文本: 首行不对"
        );
        assert!(svg.contains("<svg"), "缺 svg 根元素");
        assert!(
            svg.contains("viewBox"),
            "无 viewBox 则 <img> 无法按比例缩放"
        );
        assert!(svg.contains("</svg>"), "SVG 未闭合");

        // 确定性：同输入两次渲染必须逐字节一致（用户前后两次点同一动作
        // 看到不同的码会立刻失去信任）
        let again = sk_clip_qr_svg("https://example.com/rust".into()).expect("二次渲染失败");
        assert_eq!(svg, again, "同一输入的二维码必须逐字节一致");
    }

    #[test]
    fn qr_svg_rejects_oversized_payload() {
        // 超过 QR 码字节容量（M 级上限约 2331 字节）必须报错，而不是
        // 悄悄降级或截断——截断后的码扫出来是另一段内容，那是静默数据损坏
        let big = "a".repeat(4000);
        assert!(sk_clip_qr_svg(big).is_err(), "超容量输入必须 Err");
    }

    /// 回归锁（无人值守轮次114）：sk_disk_scan_tree 入口必须走
    /// diskusage::validate_scan_dir 同一守卫——六类拒绝（空/非绝对/设备命名
    /// 空间/DOS 保留名/不存在/非目录）逐类命中，且入口错误文案与守卫纯函数
    /// **逐字节相等**（相等断言即单一事实源锁：tree 若复制第二份清单或漏校验，
    /// 这里先红）。拒绝发生在 SCAN_STOP.reset 之前，测试不触碰全局令牌。
    #[test]
    fn tree_entry_guard_rejects_same_six_classes_as_children() {
        // 存在但不是目录（测试二进制本身是真实的绝对文件路径）
        let exe = std::env::current_exe().expect("取测试二进制路径");
        let exe_s = exe.to_string_lossy().into_owned();
        let cases: [(&str, &str); 8] = [
            // 空与纯空白
            ("", "不能为空"),
            ("   ", "不能为空"),
            // 非绝对路径
            ("relative\\dir", "绝对路径"),
            // 设备命名空间
            (r"\\.\PhysicalDrive0", "设备命名空间"),
            // DOS 保留设备名
            (r"C:\Users\NUL\sub", "保留设备名"),
            // 不存在
            (r"C:\__syskit_no_such_dir_r114__", "不存在"),
            // 存在但不是目录
            (exe_s.as_str(), "不是目录"),
            // 带首尾空白的相对路径：入口侧 trim 语义与守卫一致
            ("  ./x  ", "绝对路径"),
        ];
        for (raw, reason) in cases {
            let via_entry = tauri::async_runtime::block_on(sk_disk_scan_tree(raw.to_string()))
                .expect_err("tree 入口必须拒绝可疑路径");
            let via_fn =
                crate::diskusage::validate_scan_dir(raw).expect_err("守卫纯函数必须拒绝同一输入");
            assert!(
                via_entry.contains(reason),
                "输入 {raw:?} 的入口错误必须命中「{reason}」: {via_entry}"
            );
            assert_eq!(
                via_entry, via_fn,
                "tree 入口与 validate_scan_dir 必须是同一事实源（错误文案逐字节相等）"
            );
        }
    }

    /// 回归锁（无人值守轮次114）：合法目录放行——守卫不得误伤 tree 的正常
    /// 使用（用户自输绝对路径只读扫描），入口返回可序列化的扫描结果。
    #[test]
    fn tree_entry_accepts_real_dir_and_scans() {
        let root = std::env::temp_dir().join(format!("syskit_rs_tree_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), b"hello").unwrap();
        // SCAN_STOP 是命令共用的全局令牌：与其它可能扫描的测试串行
        let _g = crate::test_support::serial();
        let v =
            tauri::async_runtime::block_on(sk_disk_scan_tree(root.to_string_lossy().to_string()))
                .expect("真实目录必须放行并完成扫描");
        let v = v.as_object().expect("返回必须是 JSON 对象");
        let name = v["root"]["name"].as_str().expect("root.name 必须是字符串");
        assert_eq!(name, root.file_name().unwrap().to_string_lossy().as_ref());
        let _ = std::fs::remove_dir_all(&root);
    }
}
