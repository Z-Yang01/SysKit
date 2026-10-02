//! 托盘：状态色圆点图标（代码绘制，无额外依赖）、右键菜单、气泡通知。
//!
//! 与 Python 侧 ui/tray.py 对齐：菜单项、图标状态色、「最近剪贴板」子菜单。
//! M1 阶段托盘负责「打开主界面 / 立即检测 / 快速修复 / 呼出两个面板 / 退出」；
//! 状态灯与实时速率 tooltip 属 M2 遥测，接口（set_state / update_rate）先留好。

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::clipboard_history as hist;

/// 状态色（与 Python STATE_COLORS 逐值对齐）。
pub fn state_color(state: &str) -> [u8; 3] {
    match state {
        "ok" => [0x2e, 0x9e, 0x44],
        "no_net" | "ncsi_false" => [0xf2, 0xa3, 0x3c],
        "no_internet" => [0xd9, 0x40, 0x40],
        _ => [0x9e, 0x9e, 0x9e], // offline / 未知
    }
}

/// 菜单 id（tray 事件分派用）。
pub const MENU_OPEN: &str = "sk_open";
pub const MENU_CHECK: &str = "sk_check";
pub const MENU_FIX: &str = "sk_fix";
pub const MENU_PASTE: &str = "sk_paste";
pub const MENU_PALETTE: &str = "sk_palette";
pub const MENU_QUIT: &str = "sk_quit";

/// 生成 64×64 RGBA 状态圆点（带高光），返回 (rgba, size)。
///
/// 与 Python 端 QPainter 画的圆点同构：纯代码绘制，避免为托盘色点引图标资源。
pub fn state_icon_rgba(state: &str, size: u32) -> (Vec<u8>, u32) {
    let base = state_color(state);
    let mut buf = vec![0u8; (size * size * 4) as usize];
    let c = size as f32 / 2.0;
    let r = size as f32 / 2.0 - 4.0;
    // 高光圆心与半径（Python: drawEllipse(size//5, size//6, size//3, size//4)）
    let hl_cx = size as f32 / 5.0 + size as f32 / 6.0;
    let hl_cy = size as f32 / 6.0 + size as f32 / 8.0;
    let hl_r = size as f32 / 6.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - c;
            let dy = y as f32 + 0.5 - c;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > r {
                continue; // 圆外透明
            }
            let mut px = base;
            if dist > r - 2.0 {
                // 边缘 40/255 黑色描边（Python QPen(QColor(0,0,0,40), 2)）
                px = [
                    ((base[0] as f32 * 0.84) as u8),
                    ((base[1] as f32 * 0.84) as u8),
                    ((base[2] as f32 * 0.84) as u8),
                ];
            }
            let hdx = x as f32 + 0.5 - hl_cx;
            let hdy = y as f32 + 0.5 - hl_cy;
            if (hdx * hdx + hdy * hdy).sqrt() <= hl_r {
                // 高光：白色 70/255 alpha 叠加
                let a = 70.0 / 255.0;
                px = [
                    (px[0] as f32 * (1.0 - a) + 255.0 * a) as u8,
                    (px[1] as f32 * (1.0 - a) + 255.0 * a) as u8,
                    (px[2] as f32 * (1.0 - a) + 255.0 * a) as u8,
                ];
            }
            let i = ((y * size + x) * 4) as usize;
            buf[i] = px[0];
            buf[i + 1] = px[1];
            buf[i + 2] = px[2];
            buf[i + 3] = 255;
        }
    }
    (buf, size)
}

/// 装载托盘图标与菜单（setup 阶段调用一次）。
pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, MENU_OPEN, "打开主界面", true, None::<&str>)?;
    let check = MenuItem::with_id(app, MENU_CHECK, "立即检测", true, None::<&str>)?;
    let fix = MenuItem::with_id(app, MENU_FIX, "快速修复", true, None::<&str>)?;
    let paste = MenuItem::with_id(app, MENU_PASTE, "📋 快捷粘贴面板…", true, None::<&str>)?;
    let palette = MenuItem::with_id(app, MENU_PALETTE, "⌘ 命令面板", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;
    // 「最近剪贴板」子菜单：M1 先用固定条目占位（真实条目在对话框内取）
    let clip = Submenu::with_id(app, "sk_clip", "📋 最近剪贴板", true)?;
    let clip_empty =
        MenuItem::with_id(app, "sk_clip_empty", "（M1 待接历史）", false, None::<&str>)?;
    clip.append(&clip_empty)?;

    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &open, &check, &fix, &clip, &paste, &palette, &sep1, &sep2, &quit,
        ],
    )?;

    let (rgba, size) = state_icon_rgba("offline", 64);
    let icon = tauri::image::Image::new_owned(rgba, size, size);

    TrayIconBuilder::with_id("sk_tray")
        .icon(icon)
        .tooltip(base_tooltip("offline"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event: MenuEvent| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 菜单事件分派。
fn on_menu<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        MENU_OPEN => show_main(app),
        MENU_PASTE => crate::input_panel::toggle_paste_panel(app),
        MENU_PALETTE => crate::input_panel::toggle_palette(app),
        MENU_QUIT => {
            app.exit(0);
        }
        // M2/M3 落地前：告诉前端「尚未实现」，绝不静默什么都不做
        MENU_CHECK => notify(app, "立即检测", "网络检测将在 M2 遥测里程碑提供"),
        MENU_FIX => notify(app, "快速修复", "修复动作将在 M3 里程碑提供"),
        _ => {}
    }
}

pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 用托盘气泡 + 前端提示双通道告知（窗口不存在时只发事件）。
pub fn notify<R: Runtime>(app: &AppHandle<R>, title: &str, body: &str) {
    let _ = app.emit(
        "sk://notify",
        serde_json::json!({ "title": title, "body": body }),
    );
}

/// 刷新历史（供 M1+ 命令调用）：把内存态立即密封落盘。
pub fn flush_history() {
    hist::with(|h| h.flush_if_dirty());
}

/// 悬浮提示文本：`基础文案` + 可选实时速率行；与上一份相同则返回空串（= 无需更新）。
///
/// 去重是重点（对齐 Python `TrayHost._refresh_tip`）：遥测每秒回调一次，空闲时
/// 速率文本压根没变，不去重就会每秒重挂一次原生提示（NIM_MODIFY 是跨进程 shell
/// 调用，挂机一天就是 86400 次空转）。
pub fn tooltip_text(base: &str, rate: Option<&str>, last: &str) -> String {
    let candidate = match rate {
        Some(r) if !r.is_empty() => format!("{base}\n{r}"),
        _ => base.to_string(),
    };
    if candidate == last {
        String::new()
    } else {
        candidate
    }
}

/// 速率行文本（托盘 tooltip 用），对齐 Python `update_rate`。
pub fn rate_line(down_bps: u64, up_bps: u64) -> String {
    format!(
        "↓ {}/s   ↑ {}/s",
        crate::telemetry::human_bytes(down_bps as f64),
        crate::telemetry::human_bytes(up_bps as f64)
    )
}

/// 状态 → 基础提示文案（与 Python `TrayHost.set_state` 的 tips 表一致）。
pub fn base_tooltip(state: &str) -> String {
    let what = match state {
        "ok" => "网络正常",
        "no_net" => "已连接但无法上网",
        "ncsi_false" => "NCSI 误报",
        "no_internet" => "外网不可达",
        "offline" => "无网络连接",
        other => other,
    };
    format!("{} — {}", crate::paths::DISPLAY_NAME, what)
}

/// 按状态更新托盘图标与悬浮提示（由网络监控/M3 调用）。
pub fn apply_state<R: Runtime>(app: &AppHandle<R>, state: &str) {
    let Some(tray) = app.tray_by_id("sk_tray") else {
        return;
    };
    let (rgba, size) = state_icon_rgba(state, 64);
    let _ = tray.set_icon(Some(tauri::image::Image::new_owned(rgba, size, size)));
    let _ = tray.set_tooltip(Some(base_tooltip(state)));
}

/// 更新实时速率悬浮提示（尊重 tray_rate_enabled 设置；去重后写入）。
pub fn apply_rate<R: Runtime>(app: &AppHandle<R>, down_bps: u64, up_bps: u64, state: &str) {
    if !crate::settings::get_bool("tray_rate_enabled") {
        return;
    }
    let Some(tray) = app.tray_by_id("sk_tray") else {
        return;
    };
    let text = tooltip_text(&base_tooltip(state), Some(&rate_line(down_bps, up_bps)), "");
    if !text.is_empty() {
        let _ = tray.set_tooltip(Some(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_colors_match_python() {
        assert_eq!(state_color("ok"), [0x2e, 0x9e, 0x44]);
        assert_eq!(state_color("no_net"), [0xf2, 0xa3, 0x3c]);
        assert_eq!(state_color("ncsi_false"), [0xf2, 0xa3, 0x3c]);
        assert_eq!(state_color("no_internet"), [0xd9, 0x40, 0x40]);
        assert_eq!(state_color("offline"), [0x9e, 0x9e, 0x9e]);
        assert_eq!(state_color("什么鬼"), [0x9e, 0x9e, 0x9e], "未知状态回退灰");
    }

    #[test]
    fn icon_is_square_rgba_with_alpha() {
        let (buf, size) = state_icon_rgba("ok", 64);
        assert_eq!(size, 64);
        assert_eq!(buf.len(), 64 * 64 * 4);
        // 四角必须在圆外 → 全透明
        for (x, y) in [(0u32, 0u32), (63, 0), (0, 63), (63, 63)] {
            let i = ((y * 64 + x) * 4) as usize;
            assert_eq!(buf[i + 3], 0, "角点 ({x},{y}) 应透明");
        }
        // 圆心必须不透明且接近状态色
        let i = ((32 * 64 + 32) * 4) as usize;
        assert_eq!(buf[i + 3], 255);
        assert!(buf[i] > 0);
    }

    #[test]
    fn icon_radius_leaves_margin() {
        let (buf, size) = state_icon_rgba("offline", 32);
        // 圆半径 = size/2 - 4 → 最外圈一行/列应透明
        for k in 0..size {
            let top = (k * 4) as usize;
            let left = (k * size * 4) as usize;
            assert_eq!(buf[top + 3], 0, "顶行必须透明（留边）");
            assert_eq!(buf[left + 3], 0, "左列必须透明（留边）");
        }
    }

    #[test]
    fn tooltip_dedup_returns_empty_when_unchanged() {
        let base = base_tooltip("ok");
        let rate = rate_line(1024, 512);
        let first = tooltip_text(&base, Some(&rate), "");
        assert!(first.contains("网络正常"));
        assert!(first.contains("↓ 1.0 KB/s"));
        assert!(first.contains("↑ 512 B/s"));
        assert_eq!(
            tooltip_text(&base, Some(&rate), &first),
            "",
            "与上一份相同必须返回空串（否则每秒重挂原生提示）"
        );
        // 速率变了就要更新
        let rate2 = rate_line(2048, 512);
        assert!(!tooltip_text(&base, Some(&rate2), &first).is_empty());
    }

    #[test]
    fn tooltip_without_rate_is_single_line() {
        let base = base_tooltip("offline");
        assert_eq!(tooltip_text(&base, None, ""), base);
        assert!(!tooltip_text(&base, None, "").contains('\n'));
        // 空速率串等同无速率
        assert_eq!(tooltip_text(&base, Some(""), ""), base);
    }

    #[test]
    fn base_tooltips_match_python_tips_table() {
        assert!(base_tooltip("ok").ends_with("网络正常"));
        assert!(base_tooltip("no_net").ends_with("已连接但无法上网"));
        assert!(base_tooltip("ncsi_false").ends_with("NCSI 误报"));
        assert!(base_tooltip("no_internet").ends_with("外网不可达"));
        assert!(base_tooltip("offline").ends_with("无网络连接"));
        assert!(
            base_tooltip("自定义态").ends_with("自定义态"),
            "未知状态原样显示"
        );
    }

    #[test]
    fn rate_line_formats_both_directions() {
        assert_eq!(rate_line(0, 0), "↓ 0 B/s   ↑ 0 B/s");
        let l = rate_line(1024 * 1024, 1024);
        assert!(l.starts_with("↓ 1.0 MB/s"));
        assert!(l.contains("↑ 1.0 KB/s"));
    }
}
