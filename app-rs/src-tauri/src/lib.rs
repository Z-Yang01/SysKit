//! SysKit Rust 后端（M0 骨架）：Tauri Builder + 最小命令通路。
//!
//! 后续里程碑在此聚合：M1 热键/SendInput/DPAPI 原语模块、M2 遥测任务、
//! M3+ 各业务域。教训清单见 app-rs/README.md（M1 起点必读）。

/// 探活命令（M0 通路验证；前端 App.tsx 的 ping 按钮调用）。
#[tauri::command]
fn ping() -> String {
    "pong".to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ping])
        .run(tauri::generate_context!())
        .expect("SysKit 启动失败");
}

#[cfg(test)]
mod tests {
    #[test]
    fn ping_is_pong() {
        assert_eq!(super::ping(), "pong");
    }
}
