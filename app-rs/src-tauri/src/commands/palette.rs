//! 命令面板 provider：查询（算式直算 + 内置/自定义动作排序）+ 动作全集 + 呼出开关。

// ---------------- M7 命令面板 provider ----------------

/// 应用内 Ctrl+K 呼出/收起命令面板（对齐全局热键的行为）。
///
/// README 宣称「应用内 Ctrl+K」但此前主窗没有任何处理（CDP 实锤：dispatch
/// Ctrl+K 后 /json 无新 target）——Python 版有此行为，这里补齐：主窗键盘
/// 事件经前端 invoke 到这，复用与全局热键同一条 toggle_palette 链路
/// （定位到光标所在显示器、show+focus、发 sk://panel-shown）。
#[tauri::command]
pub fn sk_palette_toggle(app: tauri::AppHandle) {
    crate::input_panel::toggle_palette(&app);
}

/// 面板动作全集：内置页面跳转 + 自定义命令，按查询排序。
///
/// 匹配与排序在 Rust 侧做（与 Python 同口径），前端只渲染结果——这样两版的
/// **同一个查询排出同样的顺序**，用户不会觉得"搜索结果变了"。
#[tauri::command]
pub fn sk_palette_actions(query: Option<String>) -> serde_json::Value {
    let q = query.unwrap_or_default();
    let mut actions = builtin_actions();
    let raw = crate::settings::get_str("palette_commands");
    let cmds = crate::palette::parse_custom_commands(&raw);
    actions.extend(crate::palette::custom_to_actions(&cmds));
    let ranked = crate::palette::rank_actions(&q, &actions);
    serde_json::json!({
        "rows": ranked,
        "count": ranked.len(),
        "total": actions.len(),
        "custom_count": cmds.len(),
    })
}

/// 内置动作：v1.4 简约化后与 crate::palette::builtin_actions 单源对齐
/// （侧栏 6 页 + 13 个页内 tab 深链）。此前这里维护过一份独立的 10 项
/// 旧表（含已撤并的 clipboard/logs 独立页），是双源漂移的事故源。
fn builtin_actions() -> Vec<crate::palette::PaletteAction> {
    crate::palette::builtin_actions()
}
