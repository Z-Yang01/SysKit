//! 托盘：状态色圆点图标（代码绘制，无额外依赖）、右键菜单、气泡通知。
//!
//! 菜单项、图标状态色与 Python 侧 ui/tray.py 对齐；「最近剪贴板」子菜单是
//! **有意分叉**：用户反馈二级子菜单层级过深，且 Rust 侧从未真正填充条目
//! （Python 的 aboutToShow 填充在 Rust 没有对应实现，只有一条禁用占位）。
//! 剪贴板历史入口收敛到「打开主界面」——主窗剪贴板页已是完整管理界面，
//! 托盘只留一键可达的快捷操作。
//! 状态灯与实时速率 tooltip 属 M2 遥测，接口（set_state / update_rate）先留好。

use crate::MutexExt;
use std::sync::Mutex;
use tauri::menu::{IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
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

/// 托盘菜单条目的纯数据描述：普通项（id + 文案）或分隔线。
///
/// 抽出来是因为菜单构建内联在 `setup` 里、依赖 `AppHandle`，单元测试够不着；
/// 有了这张描述表，构建逻辑只剩机械展开，测试直接断言扁平结构（项数/顺序/
/// 文案/无子菜单），构建函数本身不需要挂着 App 才能验证。
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MenuEntry {
    /// 普通菜单项：id + 文案
    Item {
        id: &'static str,
        label: &'static str,
    },
    /// 分隔线
    Separator,
}

/// 右键菜单的扁平结构（用户反馈：「📋 最近剪贴板」二级子菜单层级过深）。
///
/// 剪贴板历史不再进托盘菜单：历史条目列表、搜索、置顶、动作识别等已由主窗
/// 剪贴板页完整承接，托盘只保留「打开主界面」这一层入口。
pub fn menu_entries() -> &'static [MenuEntry] {
    &[
        MenuEntry::Item {
            id: MENU_OPEN,
            label: "打开主界面",
        },
        MenuEntry::Item {
            id: MENU_CHECK,
            label: "立即检测",
        },
        MenuEntry::Item {
            id: MENU_FIX,
            label: "快速修复",
        },
        MenuEntry::Separator,
        MenuEntry::Item {
            id: MENU_PASTE,
            label: "📋 快捷粘贴面板…",
        },
        MenuEntry::Item {
            id: MENU_PALETTE,
            label: "⌘ 命令面板",
        },
        MenuEntry::Separator,
        MenuEntry::Item {
            id: MENU_QUIT,
            label: "退出",
        },
    ]
}

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
    // 从纯数据描述表机械展开：普通项 → MenuItem，分隔线 → PredefinedMenuItem。
    // 菜单本体是扁平列表（menu_entries），这里没有子菜单层级可言。
    let mut items: Vec<Box<dyn IsMenuItem<R>>> = Vec::new();
    for entry in menu_entries() {
        match *entry {
            MenuEntry::Item { id, label } => {
                items.push(Box::new(MenuItem::with_id(
                    app,
                    id,
                    label,
                    true,
                    None::<&str>,
                )?));
            }
            MenuEntry::Separator => {
                items.push(Box::new(PredefinedMenuItem::separator(app)?));
            }
        }
    }
    let refs: Vec<&dyn IsMenuItem<R>> = items.iter().map(|b| b.as_ref()).collect();
    let menu = Menu::with_items(app, &refs)?;

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
            // 退出前刷盘（对齐 Python quit 链的 tele.stop→落盘语义）：
            // 1) 流量/多粒度桶——遥测 60s 才落一次盘，且其循环收尾 flush 在
            //    进程退出（线程被杀）时不会执行，不在这里补就直接丢最近一分钟；
            // 2) 剪贴板历史——防抖窗口内的条目。
            if let Some(hub) = app.try_state::<std::sync::Arc<crate::telemetry_hub::Hub>>() {
                hub.flush();
            }
            crate::clipboard_history::with(|h| h.flush_if_dirty());
            app.exit(0);
        }
        // M2/M3 已交付，占位提示是过期行为：接线到真实能力（绝不静默什么都不做）。
        // 立即检测：唤醒监控循环立刻探一轮，托盘状态灯/监控页随之刷新；
        // 完整诊断页驱动（Python run_diagnosis 的 UI 部分）留前端事件通道——见上报决策。
        MENU_CHECK => {
            crate::monitor::force_check();
            notify(
                app,
                "立即检测",
                "已触发一次连通性检测，状态见托盘图标/悬浮提示",
            );
        }
        MENU_FIX => {
            // 对齐 Python act_fix → run_repair("flush_dns")：SAFE 动作直接执行，
            // 免确认。执行在阻塞线程池做（ipconfig /flushdns 秒级，不占主线程）。
            let h = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let Some(a) = crate::repairs::get_action("flush_dns") else {
                    return;
                };
                let out = a.execute(&std::collections::BTreeMap::new());
                notify(
                    &h,
                    "快速修复",
                    &format!(
                        "刷新 DNS 缓存：{}",
                        if out.success { "完成" } else { "失败" }
                    ),
                );
            });
        }
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

/// 最近一次写进托盘的速率 tooltip（去重缓存）。
///
/// 遥测每秒回调一次 apply_rate，空闲时速率文本不变——`tooltip_text` 的去重
/// 只有拿到**上一份真实写过的文本**才有意义。初版把 `last` 传死空串，去重恒
/// 不命中，每秒一次 NIM_MODIFY 跨进程 shell 调用挂机空转（约 86400 次/天），
/// 正是 Python `TrayHost._refresh_tip` 去重要避免的事。
static LAST_RATE_TOOLTIP: Mutex<String> = Mutex::new(String::new());

/// 速率 tooltip 去重：与上一份相同返回 None（无需重挂），不同则记账并返回。
fn dedupe_rate_tooltip(candidate: String) -> Option<String> {
    let mut last = LAST_RATE_TOOLTIP.lock_ok();
    if *last == candidate {
        return None;
    }
    *last = candidate.clone();
    Some(candidate)
}

/// 清空速率 tooltip 缓存（apply_state 直接改写 tooltip 后调用，
/// 让下一拍速率行能带着新基础文案重新生效）。
fn reset_rate_tooltip_cache() {
    *LAST_RATE_TOOLTIP.lock_ok() = String::new();
}

/// 按状态更新托盘图标与悬浮提示（由网络监控/M3 调用）。
pub fn apply_state<R: Runtime>(app: &AppHandle<R>, state: &str) {
    let Some(tray) = app.tray_by_id("sk_tray") else {
        return;
    };
    let (rgba, size) = state_icon_rgba(state, 64);
    let _ = tray.set_icon(Some(tauri::image::Image::new_owned(rgba, size, size)));
    let _ = tray.set_tooltip(Some(base_tooltip(state)));
    // 基础文案已换：速率缓存作废，否则下一拍同速率会被旧缓存误判为"没变"
    reset_rate_tooltip_cache();
}

/// 更新实时速率悬浮提示（尊重 tray_rate_enabled 设置；去重后写入）。
pub fn apply_rate<R: Runtime>(app: &AppHandle<R>, down_bps: u64, up_bps: u64, state: &str) {
    if !crate::settings::get_bool("tray_rate_enabled") {
        return;
    }
    let Some(tray) = app.tray_by_id("sk_tray") else {
        return;
    };
    let candidate = tooltip_text(&base_tooltip(state), Some(&rate_line(down_bps, up_bps)), "");
    if let Some(text) = dedupe_rate_tooltip(candidate) {
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
    fn rate_tooltip_dedupe_cache_suppresses_repeats() {
        // 回归（apply_rate 去重失效）：初版把 tooltip_text 的 last 传死 ""，
        // 缓存从未生效，遥测每秒都真发一次 NIM_MODIFY（跨进程 shell 调用）。
        // 现在生产调用与测试共用 dedupe_rate_tooltip 记账。
        reset_rate_tooltip_cache();
        let base = base_tooltip("offline");
        let idle = tooltip_text(&base, Some(&rate_line(0, 0)), "");
        assert!(
            dedupe_rate_tooltip(idle.clone()).is_some(),
            "第一拍必须真的写入"
        );
        assert!(
            dedupe_rate_tooltip(idle).is_none(),
            "同速率第二拍必须被缓存拦下（空闲时每秒重挂 = 86400 次/天空转）"
        );
        let busy = tooltip_text(&base, Some(&rate_line(1024, 128)), "");
        assert!(dedupe_rate_tooltip(busy).is_some(), "速率变了必须放行");
        // apply_state 改写 tooltip 后缓存作废：同速率重新生效
        reset_rate_tooltip_cache();
        let again = tooltip_text(&base, Some(&rate_line(1024, 128)), "");
        assert!(dedupe_rate_tooltip(again).is_some(), "状态切换后必须重挂");
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

    #[test]
    fn tray_menu_is_flat_with_expected_order() {
        // 用户要求的扁平结构：打开主界面 / 立即检测 / 快速修复 / --- /
        // 快捷粘贴面板 / 命令面板 / --- / 退出。
        let entries = menu_entries();
        let items: Vec<&str> = entries
            .iter()
            .filter_map(|e| match *e {
                MenuEntry::Item { id, .. } => Some(id),
                MenuEntry::Separator => None,
            })
            .collect();
        assert_eq!(
            items,
            vec![
                MENU_OPEN,
                MENU_CHECK,
                MENU_FIX,
                MENU_PASTE,
                MENU_PALETTE,
                MENU_QUIT
            ]
        );
        // 恰好两条分隔线：快捷操作 | 面板呼出 | 退出
        let seps = entries
            .iter()
            .filter(|e| **e == MenuEntry::Separator)
            .count();
        assert_eq!(seps, 2);
    }

    #[test]
    fn tray_menu_has_no_clipboard_submenu() {
        // 回归（用户反馈）：「📋 最近剪贴板」二级子菜单层级过深，整体移除；
        // 剪贴板历史入口走「打开主界面」（主窗剪贴板页是完整管理界面）。
        for e in menu_entries() {
            if let MenuEntry::Item { label, .. } = *e {
                assert!(
                    !label.contains("最近剪贴板"),
                    "托盘菜单不得再出现「最近剪贴板」条目：{label}"
                );
            }
        }
        // 旧子菜单及其占位条目的 id 不许复活（on_menu 也从未接过这两个 id）
        for e in menu_entries() {
            if let MenuEntry::Item { id, .. } = *e {
                assert!(
                    id != "sk_clip" && id != "sk_clip_empty",
                    "子菜单占位 id 复活：{id}"
                );
            }
        }
    }

    #[test]
    fn tray_menu_item_ids_all_dispatched() {
        // 菜单里的每个普通项 id 必须是 on_menu 已接线的 MENU_* 常量，
        // 否则用户点了菜单项会静默无反应（托盘菜单没有报错渠道）。
        let known = [
            MENU_OPEN,
            MENU_CHECK,
            MENU_FIX,
            MENU_PASTE,
            MENU_PALETTE,
            MENU_QUIT,
        ];
        for e in menu_entries() {
            if let MenuEntry::Item { id, .. } = *e {
                assert!(known.contains(&id), "菜单项 {id} 没有对应的事件分派分支");
            }
        }
    }
}
