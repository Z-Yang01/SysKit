//! 设置读写 / 热键重挂 / 日志查询（M7）。

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
