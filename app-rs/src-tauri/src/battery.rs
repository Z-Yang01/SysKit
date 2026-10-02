//! 电池状态与健康度（对应 Python 侧 `BatteryTab`）。
//!
//! 两层数据：
//! - **实时状态**走 `GetSystemPowerStatus`（微秒级）：是否有电池、当前电量%、是否接着
//!   交流电、充电中；
//! - **健康度**（满充容量 / 设计容量）只能从 WMI 拿（`root/wmi` 的
//!   `BatteryStaticData` / `BatteryFullChargedCapacity`）。这一步要起 PowerShell，
//!   最坏几十秒，所以**只在用户点刷新时查**，不做轮询——与 Python 的懒加载同策略。
//!
//! 纯逻辑（WMI 输出解析、健康度分档、文案）都做成可断言的函数；真机读数只做薄封装。
//! 特别地：**没有电池不是错误**（台式机很常见），必须给出明确说明而不是空白或报错。

use serde::Serialize;

/// 实时电源状态（`GetSystemPowerStatus`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PowerStatus {
    /// 是否接着交流电
    pub ac_online: bool,
    /// 是否有电池（`BatteryFlag` 的 128 位表示"无系统电池"）
    pub has_battery: bool,
    /// 电量 0~100；未知为 None（`BatteryLifePercent == 255`）
    pub percent: Option<u8>,
    /// 是否正在充电
    pub charging: bool,
    /// 电池剩余秒数（未知为 None）
    pub seconds_left: Option<u32>,
}

impl PowerStatus {
    /// 台式机/无电池的统一表示。
    pub fn no_battery() -> Self {
        Self {
            ac_online: true,
            has_battery: false,
            percent: None,
            charging: false,
            seconds_left: None,
        }
    }
}

/// 电池健康度（来自 WMI，单位 mWh）。
///
/// 不用 `Eq`：`health_percent` 是 f64（`PartialEq` 足够，测试里也只做相等比较）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BatteryHealth {
    pub design_mwh: Option<u32>,
    pub full_mwh: Option<u32>,
    /// 满充/设计 的百分比；两者缺一为 None
    pub health_percent: Option<f64>,
}

impl BatteryHealth {
    pub fn unknown() -> Self {
        Self {
            design_mwh: None,
            full_mwh: None,
            health_percent: None,
        }
    }

    /// 是否有足够数据算健康度。
    pub fn is_available(&self) -> bool {
        self.health_percent.is_some()
    }
}

/// 计算健康度：`full / design * 100`。
///
/// 返回 None 的三种情况都要分别处理，不能混成一个 0：
/// - 任一为 None（读不到）；
/// - 任一为 0（**WMI 在没有电池时会给 0**，0/0 是 NaN，会让界面上显示 "NaN%"）；
/// - 结果超出合理范围（>150%）——多半是 WMI 给了不属于同一块电池的数据，
///   宁可显示不可用也不要给用户一个荒谬的健康度。
pub fn health_percent(design: Option<u32>, full: Option<u32>) -> Option<f64> {
    let d = design?;
    let f = full?;
    if d == 0 || f == 0 {
        return None;
    }
    let pct = (f as f64 / d as f64) * 100.0;
    if !(pct.is_finite() && (1.0..=150.0).contains(&pct)) {
        return None;
    }
    Some(pct)
}

/// 健康度 → 三档（与 Python 一致：>80 好、>60 一般、其余差）。
pub fn health_token(percent: Option<f64>) -> &'static str {
    match percent {
        None => "sub",
        Some(p) if p > 80.0 => "ok",
        Some(p) if p > 60.0 => "warn",
        _ => "err",
    }
}

/// 解析 Python 那段 CIM 脚本的 `,` 分隔输出：`设计,满充,当前电量`。
///
/// 字段可能为空串（CIM 查不到时），必须按"缺席"处理而不是 0——
/// 把空串当 0 会让健康度算出 0% 或 NaN。
pub fn parse_cim_output(text: &str) -> (Option<u32>, Option<u32>, Option<u8>) {
    let parts: Vec<&str> = text.trim().split(',').collect();
    let num = |i: usize| -> Option<u32> {
        parts
            .get(i)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok())
    };
    let remain = num(2).and_then(|v| {
        // 电量是百分比：超出 100 的一律视为无效读数
        if v <= 100 {
            Some(v as u8)
        } else {
            None
        }
    });
    (num(0), num(1), remain)
}

/// 剩余时间 → 人类可读（`2 小时 5 分`）。
pub fn human_remaining(seconds: Option<u32>) -> String {
    match seconds {
        // None 与 0 都表示"不知道"：Win32 用 0xFFFFFFFF 表示未知，
        // 但把 0 显示成"0 分"会让用户以为电池马上耗尽
        None | Some(0) => "—".to_string(),
        Some(s) => {
            let h = s / 3600;
            let m = (s % 3600) / 60;
            if h > 0 {
                format!("{h} 小时 {m} 分")
            } else {
                format!("{m} 分")
            }
        }
    }
}

/// 状态摘要文案。
pub fn power_summary(p: &PowerStatus) -> String {
    if !p.has_battery {
        return "本机未检测到电池（台式机或电池信息读取失败）".to_string();
    }
    let pct = p
        .percent
        .map(|v| format!("{v}%"))
        .unwrap_or_else(|| "电量未知".to_string());
    let src = if p.ac_online {
        "交流电"
    } else {
        "电池供电"
    };
    let chg = if p.charging { " · 充电中" } else { "" };
    format!("{pct} · {src}{chg}")
}

/// 健康度说明文案（写清数据来源，避免用户把它当成官方诊断）。
pub fn health_note(h: &BatteryHealth) -> String {
    if !h.is_available() {
        return "未能读取电池健康度（需要 WMI 的 BatteryStaticData/BatteryFullChargedCapacity；\
                部分机型/驱动不提供或需要管理员权限）"
            .to_string();
    }
    "设计容量由厂商提供，仅供估算参考；健康度 = 满充容量 / 设计容量。".to_string()
}

// ---------------- FFI ----------------

#[cfg(windows)]
mod ffi {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

    use super::PowerStatus;

    /// 读实时电源状态。失败时按"无电池"处理（未知电源状态的机器极少见）。
    pub fn read_power() -> PowerStatus {
        let mut st = SYSTEM_POWER_STATUS::default();
        // SAFETY: st 是本函数栈上的有效结构体
        if unsafe { GetSystemPowerStatus(&mut st) }.is_err() {
            return PowerStatus::no_battery();
        }
        // BatteryFlag 的 128 表示"No system battery"
        let has_battery = st.BatteryFlag != 128;
        // BatteryLifePercent 255 = 未知
        let percent = if st.BatteryLifePercent == 255 {
            None
        } else {
            Some(st.BatteryLifePercent.min(100))
        };
        // BatteryLifeTime 0xFFFFFFFF = 未知
        let seconds_left = if st.BatteryLifeTime == u32::MAX {
            None
        } else {
            Some(st.BatteryLifeTime)
        };
        PowerStatus {
            ac_online: st.ACLineStatus == 1,
            has_battery,
            percent: if has_battery { percent } else { None },
            charging: has_battery && st.BatteryFlag & 8 != 0,
            seconds_left: if has_battery { seconds_left } else { None },
        }
    }
}

#[cfg(not(windows))]
mod ffi {
    use super::PowerStatus;
    pub fn read_power() -> PowerStatus {
        PowerStatus::no_battery()
    }
}

/// 读实时电源状态。
pub fn read_power() -> PowerStatus {
    ffi::read_power()
}

/// 查询电池健康度（起 PowerShell CIM，最坏 30 秒）。
///
/// 与 Python 用同一段脚本与同一套 CIM 类名——这样两版读到的数值天然一致，
/// 迁移期对比"是不是移植错了"有个共同基准。
pub fn query_health() -> BatteryHealth {
    const SCRIPT: &str = "$d=(Get-CimInstance -Namespace root/wmi -ClassName BatteryStaticData -ErrorAction SilentlyContinue | Select-Object -First 1).DesignedCapacity; $f=(Get-CimInstance -Namespace root/wmi -ClassName BatteryFullChargedCapacity -ErrorAction SilentlyContinue | Select-Object -First 1).FullChargedCapacity; $r=(Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue | Select-Object -First 1).EstimatedChargeRemaining; @($d,$f,$r) -join ','";
    let out = crate::w32::run_capture(
        &[
            "powershell".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            SCRIPT.into(),
        ],
        30,
    );
    let Ok(out) = out else {
        return BatteryHealth::unknown();
    };
    let (design, full, _remain) = parse_cim_output(&out.stdout);
    BatteryHealth {
        design_mwh: design,
        full_mwh: full,
        health_percent: health_percent(design, full),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_percent_basic() {
        assert_eq!(health_percent(Some(50000), Some(45000)), Some(90.0));
        assert_eq!(health_percent(Some(50000), Some(25000)), Some(50.0));
    }

    #[test]
    fn health_percent_zero_is_unavailable_not_nan() {
        // WMI 在没有电池时会给 0；0/0 是 NaN，界面会显示 "NaN%"
        assert_eq!(health_percent(Some(0), Some(0)), None);
        assert_eq!(health_percent(Some(0), Some(100)), None);
        assert_eq!(health_percent(Some(100), Some(0)), None);
    }

    #[test]
    fn health_percent_missing_is_none() {
        assert_eq!(health_percent(None, Some(100)), None);
        assert_eq!(health_percent(Some(100), None), None);
        assert_eq!(health_percent(None, None), None);
    }

    #[test]
    fn health_percent_rejects_absurd_values() {
        // 满充远大于设计（比如 WMI 给了不属于同一块电池的数据）→ 宁可不显示
        assert_eq!(health_percent(Some(1000), Some(5000)), None, "500% 不可信");
        assert_eq!(health_percent(Some(100), Some(151)), None, ">150% 拒绝");
        // 边界：150% 恰好接受（换过电池的机器可能略超设计）
        assert_eq!(health_percent(Some(100), Some(150)), Some(150.0));
        // 极低但合法（电池快废了）要显示出来，不能因为"看着离谱"就吞掉
        assert_eq!(health_percent(Some(50000), Some(1000)), Some(2.0));
    }

    #[test]
    fn health_token_three_tiers_with_boundaries() {
        assert_eq!(health_token(Some(100.0)), "ok");
        assert_eq!(health_token(Some(80.1)), "ok");
        assert_eq!(
            health_token(Some(80.0)),
            "warn",
            "80 恰好不算好（Python 用 >80）"
        );
        assert_eq!(health_token(Some(60.1)), "warn");
        assert_eq!(
            health_token(Some(60.0)),
            "err",
            "60 恰好算差（Python 用 >60）"
        );
        assert_eq!(health_token(Some(10.0)), "err");
        assert_eq!(health_token(None), "sub");
    }

    #[test]
    fn parse_cim_output_well_formed() {
        assert_eq!(
            parse_cim_output("50000,45000,88"),
            (Some(50000), Some(45000), Some(88))
        );
        assert_eq!(
            parse_cim_output("50000,45000,88\r\n"),
            (Some(50000), Some(45000), Some(88))
        );
        assert_eq!(
            parse_cim_output("  50000 , 45000 , 88  "),
            (Some(50000), Some(45000), Some(88))
        );
    }

    #[test]
    fn parse_cim_output_empty_fields_are_none_not_zero() {
        // 台式机上三个字段都可能是空串；当成 0 会算出 0% 或 NaN
        assert_eq!(parse_cim_output(",,,"), (None, None, None));
        assert_eq!(parse_cim_output(""), (None, None, None));
        assert_eq!(parse_cim_output("50000,,88"), (Some(50000), None, Some(88)));
        assert_eq!(parse_cim_output(",45000,"), (None, Some(45000), None));
    }

    #[test]
    fn parse_cim_output_rejects_non_numeric_and_out_of_range_percent() {
        assert_eq!(parse_cim_output("abc,def,ghi"), (None, None, None));
        // 电量越界只让"电量"变成 None：设计/满充容量仍然有效，不能连坐丢掉
        // （丢了就再也算不出健康度，而那正是用户最想看的一项）
        assert_eq!(
            parse_cim_output("50000,45000,999"),
            (Some(50000), Some(45000), None),
            "越界电量只作废该字段"
        );
        assert_eq!(
            parse_cim_output("-5000,45000,50"),
            (None, Some(45000), Some(50)),
            "负数无效"
        );
        assert_eq!(
            parse_cim_output("50000,45000,100"),
            (Some(50000), Some(45000), Some(100))
        );
        assert_eq!(
            parse_cim_output("50000,45000,0"),
            (Some(50000), Some(45000), Some(0)),
            "0% 是合法电量"
        );
    }

    #[test]
    fn parse_cim_output_handles_powershell_error_text() {
        // 脚本出错时 stdout 里可能是别的东西（含换行）；不能 panic
        let (d, f, r) = parse_cim_output("Get-CimInstance : 拒绝访问。\r\n");
        assert_eq!((d, f), (None, None));
        assert!(r.is_none());
    }

    #[test]
    fn human_remaining_variants() {
        assert_eq!(human_remaining(Some(7200)), "2 小时 0 分");
        assert_eq!(human_remaining(Some(7500)), "2 小时 5 分");
        assert_eq!(human_remaining(Some(300)), "5 分");
        assert_eq!(human_remaining(Some(0)), "—", "0 表示未知，不该显示 0 分");
        assert_eq!(human_remaining(None), "—");
    }

    #[test]
    fn power_summary_no_battery() {
        let p = PowerStatus::no_battery();
        let s = power_summary(&p);
        assert!(s.contains("未检测到电池"), "{s}");
    }

    #[test]
    fn power_summary_on_ac_and_charging() {
        let p = PowerStatus {
            ac_online: true,
            has_battery: true,
            percent: Some(88),
            charging: true,
            seconds_left: None,
        };
        let s = power_summary(&p);
        assert!(s.contains("88%"), "{s}");
        assert!(s.contains("交流电"), "{s}");
        assert!(s.contains("充电中"), "{s}");
    }

    #[test]
    fn power_summary_on_battery_not_charging() {
        let p = PowerStatus {
            ac_online: false,
            has_battery: true,
            percent: Some(42),
            charging: false,
            seconds_left: Some(3600),
        };
        let s = power_summary(&p);
        assert!(s.contains("42%"), "{s}");
        assert!(s.contains("电池供电"), "{s}");
        assert!(!s.contains("充电中"), "{s}");
    }

    #[test]
    fn power_summary_unknown_percent() {
        let p = PowerStatus {
            ac_online: true,
            has_battery: true,
            percent: None,
            charging: false,
            seconds_left: None,
        };
        assert!(power_summary(&p).contains("电量未知"));
    }

    #[test]
    fn health_note_explains_missing_data() {
        let s = health_note(&BatteryHealth::unknown());
        assert!(s.contains("未能读取"), "{s}");
        assert!(s.contains("WMI"), "要说明依赖什么: {s}");
        // 不可用状态不得给出"健康度 = x%"式表述
        assert!(!s.contains("健康度 ="), "{s}");
    }

    #[test]
    fn health_note_when_available_mentions_source() {
        let s = health_note(&BatteryHealth {
            design_mwh: Some(50000),
            full_mwh: Some(45000),
            health_percent: Some(90.0),
        });
        assert!(s.contains("厂商"), "{s}");
        assert!(s.contains("参考"), "{s}");
    }

    #[test]
    fn health_is_available_flag() {
        assert!(!BatteryHealth::unknown().is_available());
        assert!(BatteryHealth {
            design_mwh: Some(1),
            full_mwh: Some(1),
            health_percent: Some(100.0),
        }
        .is_available());
    }

    #[test]
    fn live_power_status_is_self_consistent() {
        let p = read_power();
        if p.has_battery {
            if let Some(pct) = p.percent {
                assert!(pct <= 100);
            }
            // 没有电池时 charging 必须为 false（否则界面会显示"充电中"却无电池）
        } else {
            assert!(p.percent.is_none(), "无电池时不应有电量读数");
            assert!(!p.charging, "无电池时不应显示充电中");
            assert!(p.seconds_left.is_none());
        }
    }

    #[test]
    fn live_health_query_does_not_panic_on_desktop() {
        // 台式机：应返回"不可用"而不是 panic 或编造数值
        let h = query_health();
        if let Some(p) = h.health_percent {
            assert!(p > 0.0 && p <= 150.0, "健康度应在合理范围: {p}");
        } else {
            // 不可用时两个容量都应缺失或至少算不出健康度
            assert!(health_percent(h.design_mwh, h.full_mwh).is_none());
        }
    }
}
