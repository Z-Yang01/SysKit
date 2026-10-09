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

/// 内置动作（页面跳转）。页内 Tab 由前端按当前页面结构补充。
fn builtin_actions() -> Vec<crate::palette::PaletteAction> {
    const PAGES: [(&str, &str, &str); 10] = [
        ("nav:overview", "打开 概览", "overview"),
        ("nav:clipboard", "打开 剪贴板", "clipboard"),
        ("nav:diagnose", "打开 诊断与修复", "diagnose"),
        ("nav:toolbox", "打开 工具箱", "toolbox"),
        ("nav:system", "打开 系统助手", "system"),
        ("nav:vault", "打开 密码库", "vault"),
        ("nav:settings", "打开 设置 / 日志", "settings"),
        ("nav:hosts", "打开 Hosts 编辑", "toolbox"),
        ("nav:ports", "打开 端口 / 局域网", "toolbox"),
        ("nav:dns", "打开 DNS / 子网 / ping", "toolbox"),
    ];
    PAGES
        .iter()
        .map(|(id, label, page)| crate::palette::PaletteAction {
            id: (*id).to_string(),
            label: (*label).to_string(),
            hint: "页面跳转".to_string(),
            keywords: vec![
                "goto".to_string(),
                "页面".to_string(),
                "打开".to_string(),
                (*page).to_string(),
            ],
            source: "内置".to_string(),
        })
        .collect()
}
