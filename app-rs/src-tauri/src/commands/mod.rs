//! Tauri 前端可调命令（`invoke` 接口）。所有命令都是薄封装，业务逻辑在各模块里。

use crate::clipboard_history::ClipItem;
use crate::settings;
use crate::MutexExt;

// ---------------- 按域拆分的子模块（v1.3 架构优化，渐进迁移） ----------------
// re-export 保持 lib.rs 的 commands::sk_xxx 引用不变；尚未拆出的域
// （诊断修复/工具箱杂项/剪贴板/设置日志/系统助手/命令面板）仍在本文件。
mod network;
pub use network::*;
mod toolbox;
pub use toolbox::*;
mod diag;
pub use diag::*;
mod clipboard_cmds;
mod settings_cmds;
mod system;
mod vault;
pub use clipboard_cmds::*;
mod palette;
pub use palette::*;
pub use settings_cmds::*;
pub use system::*;
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
            let mut c = crate::vault::crypto::VaultCrypto::with_path(
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
            "clipboard_cmds.rs",
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
                crate::disk::usage::validate_scan_dir(raw).expect_err("守卫纯函数必须拒绝同一输入");
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
