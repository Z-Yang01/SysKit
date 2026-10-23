//! 系统助手：场景模式 / 电池 / 磁盘与清理 / 启动项 / Top 进程 / WiFi 状态（M5）。
use super::*;

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
    let root = crate::disk::usage::validate_scan_dir(&root)?;
    crate::disk::usage::SCAN_STOP.reset();
    tauri::async_runtime::spawn_blocking(move || {
        let r = crate::disk::usage::scan_tree(&root, &crate::disk::usage::SCAN_STOP);
        serde_json::to_value(r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

/// 取消进行中的目录扫描（已入回收站的删除不受影响）。
#[tauri::command]
pub fn sk_disk_scan_cancel() {
    crate::disk::usage::SCAN_STOP.cancel();
}

/// 目录逐级展开：扫描一个目录的直接子项（递归大小/类型分布/Top200 摘要，
/// 只返回一层——孙辈由前端再次调用按需下钻）。路径校验见
/// diskusage::validate_scan_dir（绝对路径/存在且为目录/拒绝设备命名空间与
/// DOS 保留设备名）。取消走 sk_disk_scan_cancel（与 sk_disk_scan_tree
/// 同一枚 SCAN_STOP，同一时刻只跑一个扫描）。
#[tauri::command]
pub async fn sk_disk_scan_children(dir: String) -> Result<serde_json::Value, String> {
    let dir = crate::disk::usage::validate_scan_dir(&dir)?;
    crate::disk::usage::SCAN_STOP.reset();
    tauri::async_runtime::spawn_blocking(move || {
        let r = crate::disk::usage::scan_children(&dir, &crate::disk::usage::SCAN_STOP);
        serde_json::to_value(r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

/// 顽固文件/文件夹强制删除（流氓软件残留）：四阶段管线（属性→直接删→
/// takeown→icacls→强删+重试）。不经回收站、不可还原——前端必须二次确认。
/// 需要管理员（takeown/icacls 对非自有文件生效的前提）。
#[tauri::command]
pub async fn sk_force_delete(paths: Vec<String>) -> Result<serde_json::Value, String> {
    if !crate::w32::is_elevated() {
        return Err("强制删除需要管理员权限：takeown/icacls 重置所有权必须提权运行。请以管理员身份重启 SysKit 后重试。".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let results: Vec<crate::disk::stubborn::ForceResult> = paths
            .iter()
            .map(|p| crate::disk::stubborn::force_remove(p))
            .collect();
        let ok_count = results.iter().filter(|r| r.ok).count();
        serde_json::json!({ "ok_count": ok_count, "results": results })
    })
    .await
    .map_err(|e| format!("删除线程失败: {e}"))
}

/// 批量移入回收站（可还原），返回 (成功数, 失败路径列表)。
#[tauri::command]
pub async fn sk_disk_recycle_delete(paths: Vec<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (ok, errors) = crate::disk::usage::delete_to_recycle_bin(&paths);
        serde_json::json!({ "ok_count": ok, "errors": errors })
    })
    .await
    .map_err(|e| format!("删除线程失败: {e}"))
}

/// 列出本地卷的容量与使用率。
#[tauri::command]
pub async fn sk_disk_list() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let drives = crate::disk::overview::sort_by_pressure(crate::disk::overview::list_drives());
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
                    "token": crate::disk::overview::usage_token(d.used_percent()),
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
                    "human": crate::disk::overview::human_size(bytes),
                    "roots": t.roots().iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(serde_json::json!({
            "rows": rows,
            "total_bytes": total,
            "total_human": crate::disk::overview::human_size(total),
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

/// Top 进程缓存（2 秒窗口内复用同一份，前端 3 秒轮询时大多直接命中）。
static TOP_PROCS_CACHE: std::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>> =
    std::sync::Mutex::new(None);

/// 内存占用最高的 N 个进程。
///
/// **数据源（2026-10-19 换）**：主路径是 `w32::snapshot_process_memory`
/// （Toolhelp + `K32GetProcessMemoryInfo`），全内存操作、约 5–15ms；
/// `tasklist /fo csv` 降级为兜底。换掉的原因不是解析规则，而是**成本**：
/// 起一个 tasklist 子进程实测 1.43 秒，而性能页每 3 秒刷一次——等于常驻
/// 半个 CPU 核，用户会直接看到"SysKit 自己很吃 CPU"。
/// 端口页仍用 tasklist CSV 解析（那里是一次性动作，且有现成规则）。
#[tauri::command]
pub async fn sk_top_procs(limit: Option<usize>) -> Result<serde_json::Value, String> {
    let n = limit.unwrap_or(15).clamp(1, 100);
    tauri::async_runtime::spawn_blocking(move || {
        const TTL_MS: u128 = 2000;
        {
            let g = TOP_PROCS_CACHE.lock_ok();
            if let Some((at, v)) = g.as_ref() {
                if at.elapsed().as_millis() < TTL_MS {
                    return Ok(v.clone());
                }
            }
        }
        // 主路径：原生 Toolhelp 快照（全内存操作，5–15ms）。
        // 兜底：tasklist 子进程（实测 1.4s）——只在原生路径整体失败时用，
        // 保证"换实现"不会把功能换没了。
        let procs: Vec<crate::portscan::ProcMem> = match crate::w32::snapshot_process_memory() {
            Some(raw) if !raw.is_empty() => raw
                .into_iter()
                .map(|p| crate::portscan::ProcMem {
                    pid: p.pid,
                    name: p.name,
                    bytes: p.working_set,
                })
                .collect(),
            _ => {
                let out = crate::w32::run_capture(&args(&["tasklist", "/fo", "csv", "/nh"]), 30)
                    .map_err(|e| format!("tasklist 无法执行: {e}"))?;
                crate::portscan::parse_tasklist_mem(&out.stdout)
            }
        };
        if procs.is_empty() {
            return Err("未能枚举出任何进程".to_string());
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
        *TOP_PROCS_CACHE.lock_ok() = Some((std::time::Instant::now(), value.clone()));
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

/// 已配对蓝牙设备清单（注册表枚举，毫秒级，不含电量）。
///
/// 与电量分开：GATT 读电量对失联设备要等到超时，混在一起会让"打开设备页"
/// 从几十毫秒变成十几秒。前端先拿清单渲染，再单独请求电量。
#[tauri::command]
pub async fn sk_devices_list() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        serde_json::json!({ "devices": crate::devices::list_paired() })
    })
    .await
    .map_err(|e| format!("设备枚举线程失败: {e}"))
}

/// 批量探测：连接状态 + 电量，并行，每个设备独立 4 秒超时。
///
/// 返回 `{ "probe": { "AA:BB:..": { connected: bool, battery: 87|null } } }`；
/// 某键缺失 = 这次探测超时/失败（设备失联），与 `battery: null`
/// （设备在、但不上报电量）是两回事，界面要能区分。
#[tauri::command]
pub async fn sk_devices_battery(addrs: Vec<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let map = crate::devices::probes(&addrs, std::time::Duration::from_secs(3));
        serde_json::json!({ "probe": map })
    })
    .await
    .map_err(|e| format!("设备探测线程失败: {e}"))
}

/// 蓝牙无线电状态：`available=false` = 本机没有蓝牙适配器。
#[tauri::command]
pub async fn sk_bt_status() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| match crate::devices::radio_state() {
        Some(on) => serde_json::json!({ "available": true, "on": on }),
        None => serde_json::json!({ "available": false, "on": false }),
    })
    .await
    .map_err(|e| format!("蓝牙状态线程失败: {e}"))
}

/// 打开系统蓝牙设置（固定目标，不接受任意协议串——与 sk_open_url 的白名单同理）。
#[tauri::command]
pub fn sk_open_bt_settings() -> Result<(), String> {
    crate::w32::shell_open("ms-settings:bluetooth")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
