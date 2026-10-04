//! WiFi 信号与 SSID：走 `wlanapi.dll`（`WlanOpenHandle`/`WlanQueryInterface`）。
//!
//! ## 为什么不用 `netsh wlan show interfaces`
//!
//! Python 版每 5 秒起一次 `netsh` 子进程读信号与 SSID。Rust 侧改用 `wlanapi.dll`：
//! 一次调用约几十微秒，而子进程创建是毫秒级且要解析本地化文本（中英输出格式不同，
//! 信号行还有 `信号` / `Signal` 两种写法）。更重要的是——**SSID 决定流量归因桶**
//! （`rate_stats` 的 WiFi 分桶），解析失败就会静默归到「以太网/未知」。
//! 直接从 API 拿 `dot11Ssid` 字节串，没有解析环节就没有解析失败。
//!
//! 连接状态用 `isState == wlan_interface_state_connected` 判定，**不是**靠信号值
//! 是否为空：信号质量为 0 也是有效读数（贴着覆盖边缘），把它当成"未连接"
//! 会让用户看到 SSID 存在却报断开。

/// 一次 WiFi 读数。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WifiInfo {
    /// 是否已连接
    pub connected: bool,
    /// SSID（未连接为空）
    pub ssid: String,
    /// 信号质量 0~100（未连接为 None）
    pub signal: Option<u32>,
    /// 估算 dBm（未连接为 None）
    pub dbm: Option<i32>,
    /// 关联的 BSSID（如 `aa:bb:cc:dd:ee:ff`）
    pub bssid: String,
    /// 接收/发送链路速率（kbps）
    pub rx_kbps: u32,
    pub tx_kbps: u32,
}

impl WifiInfo {
    /// 未连接/无无线网卡的统一表示。
    pub fn disconnected() -> Self {
        Self {
            connected: false,
            ssid: String::new(),
            signal: None,
            dbm: None,
            bssid: String::new(),
            rx_kbps: 0,
            tx_kbps: 0,
        }
    }
}

/// 信号质量(0~100) → 估算 dBm。
///
/// 与 Windows 自己的换算一致：`quality = 2 * (dBm + 100)`，反解 `dBm = quality/2 - 100`。
/// 质量 0 在真实世界里到不了（约 -100 dBm 已完全不可用），所以这里不额外裁剪下界。
pub fn quality_to_dbm(quality: u32) -> i32 {
    (quality.min(100) as i32) / 2 - 100
}

/// 信号质量 → 展示文案分档（弱/一般/良好/极佳）。
pub fn signal_label(quality: u32) -> &'static str {
    match quality {
        0..=24 => "很弱",
        25..=49 => "一般",
        50..=74 => "良好",
        _ => "极佳",
    }
}

/// SSID 字节串 → 字符串（`dot11Ssid` 是定长数组 + 长度，不是 NUL 结尾）。
///
/// **必须按 ucSSIDLength 截断**：数组后面的字节是残留垃圾，按 NUL 结尾读会把
/// 上一次调用的内容拼进 SSID 里——用户会看到乱码网络名，而流量也因此归错桶。
pub fn ssid_from_bytes(bytes: &[u8], len: usize) -> String {
    let n = len.min(bytes.len());
    String::from_utf8_lossy(&bytes[..n]).to_string()
}

/// MAC 字节 → `aa:bb:cc:dd:ee:ff`。
pub fn format_bssid(bytes: &[u8; 6]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

// ---------------- FFI ----------------

#[cfg(windows)]
mod ffi {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::WiFi::{
        wlan_interface_state_connected, wlan_intf_opcode_current_connection, WlanCloseHandle,
        WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle, WlanQueryInterface,
        WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST,
    };

    use super::{format_bssid, quality_to_dbm, ssid_from_bytes, WifiInfo};

    /// 读取当前 WiFi 状态。任何一步失败都返回"未连接"而不是报错——
    /// 无无线网卡的台式机上这是正常状态，不该让调用方处理异常。
    pub fn read() -> WifiInfo {
        // SAFETY: 每步都检查返回码；所有由 API 分配的指针都用 WlanFreeMemory 释放。
        unsafe {
            let mut negotiated = 0u32;
            let mut handle = HANDLE::default();
            // 版本 2 = 客户端版本（Win Vista+ 均可用）
            if WlanOpenHandle(2, None, &mut negotiated, &mut handle) != 0 {
                return WifiInfo::disconnected();
            }
            // 用 guard 保证任何提前返回都关闭句柄（否则每次采样漏一个句柄，
            // 长时间运行会耗尽——Python 版因为用子进程不存在这个问题）
            let _guard = HandleGuard(handle);

            let mut list_ptr: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            if WlanEnumInterfaces(handle, None, &mut list_ptr) != 0 || list_ptr.is_null() {
                return WifiInfo::disconnected();
            }
            let _list_guard = MemGuard(list_ptr as *const core::ffi::c_void);

            let list = &*list_ptr;
            if list.dwNumberOfItems == 0 {
                return WifiInfo::disconnected();
            }
            // 用第一个无线接口（多网卡机器上通常也只有一个 WLAN 接口）
            let guid = list.InterfaceInfo[0].InterfaceGuid;

            let mut size = 0u32;
            let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut opcode_type = Default::default();
            if WlanQueryInterface(
                handle,
                &guid,
                wlan_intf_opcode_current_connection,
                None,
                &mut size,
                &mut data,
                Some(&mut opcode_type),
            ) != 0
                || data.is_null()
            {
                // 未连接时该 opcode 会失败——正常的"没连 WiFi"
                return WifiInfo::disconnected();
            }
            let _data_guard = MemGuard(data);

            let attrs = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
            if attrs.isState != wlan_interface_state_connected {
                return WifiInfo::disconnected();
            }
            let assoc = &attrs.wlanAssociationAttributes;
            let ssid = ssid_from_bytes(
                &assoc.dot11Ssid.ucSSID,
                assoc.dot11Ssid.uSSIDLength as usize,
            );
            let quality = assoc.wlanSignalQuality.min(100);
            WifiInfo {
                connected: true,
                ssid,
                signal: Some(quality),
                dbm: Some(quality_to_dbm(quality)),
                bssid: format_bssid(&assoc.dot11Bssid),
                rx_kbps: assoc.ulRxRate,
                tx_kbps: assoc.ulTxRate,
            }
        }
    }

    /// 保证句柄一定被关闭。
    struct HandleGuard(HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            // SAFETY: 句柄由 WlanOpenHandle 成功返回，只在这里关闭一次
            unsafe {
                let _ = WlanCloseHandle(self.0, None);
            }
        }
    }

    /// 保证 API 分配的内存一定被释放。
    struct MemGuard(*const core::ffi::c_void);
    impl Drop for MemGuard {
        fn drop(&mut self) {
            // SAFETY: 指针来自 WlanEnumInterfaces/WlanQueryInterface，只释放一次
            unsafe { WlanFreeMemory(self.0) }
        }
    }
}

#[cfg(not(windows))]
mod ffi {
    use super::WifiInfo;
    pub fn read() -> WifiInfo {
        WifiInfo::disconnected()
    }
}

/// 读取当前 WiFi 状态（非 Windows 上恒为未连接）。
pub fn read() -> WifiInfo {
    ffi::read()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_to_dbm_matches_windows_formula() {
        // Windows: quality = 2 * (dBm + 100)
        assert_eq!(quality_to_dbm(100), -50);
        assert_eq!(quality_to_dbm(50), -75);
        assert_eq!(quality_to_dbm(0), -100);
        // 超出 100 的读数被夹住（API 不该给出，但别让换算失控）
        assert_eq!(quality_to_dbm(150), -50);
    }

    #[test]
    fn quality_to_dbm_is_lossy_for_odd_values() {
        // 换算是整数除法：`dBm = quality/2 - 100`，所以**奇数 quality 会丢 1**，
        // 反解回去比原值小 1（Windows 自己的公式同样如此）。
        // 把它写清楚，免得后人以为往返不齐是 bug 而去"修"它。
        for q in [0u32, 10, 50, 100] {
            let back = 2 * (quality_to_dbm(q) + 100);
            assert_eq!(back, q as i32, "偶数 quality {q} 往返一致");
        }
        for q in [25u32, 51, 99] {
            let back = 2 * (quality_to_dbm(q) + 100);
            assert_eq!(
                back,
                q as i32 - 1,
                "奇数 quality {q} 往返少 1（整数除法的必然结果）"
            );
        }
    }

    #[test]
    fn quality_to_dbm_is_monotonic() {
        // 展示层依赖单调性：信号条不会出现"质量更高但 dBm 更低"
        let mut prev = i32::MIN;
        for q in 0..=100u32 {
            let dbm = quality_to_dbm(q);
            assert!(dbm >= prev, "quality {q} 处 dBm 应单调不减");
            prev = dbm;
        }
    }

    #[test]
    fn signal_label_boundaries() {
        assert_eq!(signal_label(0), "很弱");
        assert_eq!(signal_label(24), "很弱");
        assert_eq!(signal_label(25), "一般");
        assert_eq!(signal_label(49), "一般");
        assert_eq!(signal_label(50), "良好");
        assert_eq!(signal_label(74), "良好");
        assert_eq!(signal_label(75), "极佳");
        assert_eq!(signal_label(100), "极佳");
    }

    #[test]
    fn ssid_from_bytes_truncates_by_length_not_nul() {
        // 关键：dot11Ssid 是定长数组，后面的字节是残留垃圾。
        // 按 NUL 结尾读会把垃圾拼进 SSID → 用户看到乱码网络名 + 流量归错桶。
        let mut raw = [0u8; 32];
        raw[..5].copy_from_slice(b"MyNet");
        raw[7] = b'X'; // 长度之外的残留
        assert_eq!(ssid_from_bytes(&raw, 5), "MyNet");
        // 给足长度时才把数组内容全带出来（含残留与 NUL）——
        // 用程序化构造期望值，避免手数 NUL 个数出错
        assert_eq!(ssid_from_bytes(&raw, 32), String::from_utf8_lossy(&raw));
        assert!(
            ssid_from_bytes(&raw, 32).contains('X'),
            "残留字节确实在数组里"
        );
    }

    #[test]
    fn ssid_from_bytes_handles_utf8_and_bad_input() {
        let utf8 = "中文WiFi".as_bytes();
        let mut raw = [0u8; 32];
        raw[..utf8.len()].copy_from_slice(utf8);
        assert_eq!(ssid_from_bytes(&raw, utf8.len()), "中文WiFi");
    }

    #[test]
    fn ssid_from_bytes_clamps_oversized_length() {
        // 长度字段比数组大（不该发生）时不得越界 panic
        let raw = [b'A'; 8];
        assert_eq!(ssid_from_bytes(&raw, 999), "AAAAAAAA");
    }

    #[test]
    fn ssid_from_bytes_empty_length_gives_empty() {
        let raw = [b'Z'; 8];
        assert_eq!(ssid_from_bytes(&raw, 0), "");
    }

    #[test]
    fn hidden_ssid_is_empty_string_not_garbage() {
        // 隐藏网络：长度 0 → 空串（上游据此归「以太网/未知」桶，与 Python 一致）
        let raw = [0u8; 32];
        assert_eq!(ssid_from_bytes(&raw, 0), "");
    }

    #[test]
    fn format_bssid_lowercase_colons() {
        assert_eq!(
            format_bssid(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            "aa:bb:cc:dd:ee:ff"
        );
        assert_eq!(
            format_bssid(&[0x00, 0x01, 0x02, 0x03, 0x04, 0x05]),
            "00:01:02:03:04:05"
        );
    }

    #[test]
    fn disconnected_helper_is_consistent() {
        let d = WifiInfo::disconnected();
        assert!(!d.connected);
        assert!(d.ssid.is_empty());
        assert!(d.signal.is_none(), "未连接时信号必须是 None 而不是 0");
        assert!(d.dbm.is_none());
        assert!(d.bssid.is_empty());
    }

    #[test]
    fn read_never_panics_and_is_self_consistent() {
        // 真机读取：无网卡/未连接都必须返回一致结果，而不是 panic
        let w = read();
        if !w.connected {
            assert!(w.ssid.is_empty());
            assert!(w.signal.is_none());
            assert!(w.dbm.is_none());
        } else {
            assert!(w.signal.unwrap() <= 100, "信号质量必须 <= 100");
            // 已连接时 SSID 可能为空（隐藏网络），但信号值必须存在
            assert!(w.dbm.is_some());
            assert_eq!(w.dbm, w.signal.map(quality_to_dbm));
        }
    }

    #[test]
    fn read_twice_is_stable() {
        // 句柄泄漏的粗测：连读 200 次不应失败或变慢到超时（Drop guard 的作用）
        for _ in 0..200 {
            let _ = read();
        }
        let w = read();
        // 只要不 panic、结果自洽即可
        if w.connected {
            assert!(w.signal.is_some());
        }
    }

    #[test]
    fn wifi_info_serializes_with_expected_fields() {
        let j = serde_json::to_string(&WifiInfo::disconnected()).unwrap();
        for k in [
            "connected",
            "ssid",
            "signal",
            "dbm",
            "bssid",
            "rx_kbps",
            "tx_kbps",
        ] {
            assert!(j.contains(k), "序列化必须含 {k}: {j}");
        }
    }
}

/// 已保存的 WLAN 配置文件的凭据。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProfileCredential {
    /// 配置文件名（= SSID）
    pub name: String,
    /// 认证类型（原文，如 `WPA2-Personal` / `开放式`）
    pub auth: String,
    /// 明文密钥；取不到时为空串
    pub password: String,
}

/// 从 `netsh wlan show profile name=… key=clear` 的输出里取认证类型与明文密钥。
///
/// **中英文都要认**：netsh 输出随系统语言变化，只认一种会在另一种语言下静默
/// 返回空密码——用户看到"密码是空的"却不知道是解析失败。
///
/// 按**第一个**冒号/全角冒号切分：密码本身可能含冒号，按最后一个切会截断。
pub fn parse_profile_credential(text: &str) -> (String, String) {
    let mut auth = String::new();
    let mut pwd = String::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once([':', '：']) else {
            continue;
        };
        let k = key.trim();
        let v = value.trim();
        // 「身份验证」= Authentication；「关键内容」= Key Content
        let is_auth = k.ends_with("身份验证") || k.eq_ignore_ascii_case("Authentication");
        let is_key = k.ends_with("关键内容") || k.eq_ignore_ascii_case("Key Content");
        // 首次命中即定：后续空值不得覆盖已取到的值
        if is_auth && auth.is_empty() {
            auth = v.to_string();
        } else if is_key && pwd.is_empty() {
            pwd = v.to_string();
        }
    }
    (auth, pwd)
}

/// 读取某个已保存配置文件的凭据（需要管理员权限才能取回明文密钥）。
pub fn profile_credential(name: &str) -> Result<ProfileCredential, String> {
    let mut args: Vec<String> = ["netsh", "wlan", "show", "profile"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.push(format!("name={name}"));
    args.push("key=clear".to_string());
    let out = crate::w32::run_capture(&args, 15).map_err(|e| format!("netsh 无法执行: {e}"))?;
    let text = out.combined();
    // netsh 对不存在的配置文件会失败：明确报出来而不是给一个空密码
    if text.contains("找不到") || text.to_lowercase().contains("not found") {
        return Err(format!("找不到配置文件 {name}"));
    }
    let (auth, password) = parse_profile_credential(&text);
    let is_open = auth.contains("开放") || auth.to_lowercase().contains("open");
    if password.is_empty() && !is_open {
        // 有认证却拿不到密钥：最常见原因是没有管理员权限（key=clear 接口限制）
        return Err(
            "读取明文密钥失败——通常需要以管理员身份运行（netsh key=clear 接口限制）".to_string(),
        );
    }
    Ok(ProfileCredential {
        name: name.to_string(),
        auth,
        password,
    })
}

// ---------------- 周边 AP 扫描（netsh wlan show networks mode=bssid） ----------------

/// 一个周边 AP（BSSID 级）的扫描结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ScannedNetwork {
    /// 网络名（隐藏网络为 `(隐藏)`，与 Python 版同文案）
    pub ssid: String,
    /// 接入点 MAC（`aa:bb:cc:dd:ee:ff`）
    pub bssid: String,
    /// 信号质量 0~100（netsh 未给出时为 0）
    pub signal: u32,
    /// 信道号（0 = netsh 未给出，不编造）
    pub channel: u32,
    /// 无线电类型（如 `802.11ax`）
    pub radio: String,
    /// 认证方式原文（如 `WPA2-Personal` / `WPA2 - 个人` / `Open`）
    pub auth: String,
    /// 按信道号推导的频段：`2.4G` / `5G` / `?`
    pub band: String,
}

/// 信道号 → 频段（与 Python wifi_page 同口径：1~14 为 2.4G，>14 为 5G）。
///
/// 信道 14 只在日本 802.11b 用过，仍归 2.4G——按物理频段归类，不按可用性。
pub fn band_of_channel(channel: u32) -> &'static str {
    if channel == 0 {
        "?"
    } else if channel <= 14 {
        "2.4G"
    } else {
        "5G"
    }
}

/// 从 `netsh wlan show networks mode=bssid` 的输出解析 AP 列表。
///
/// **中英文都要认**：netsh 输出随系统语言变化（`信号`/`Signal`、`信道`/`Channel`、
/// `无线电类型`/`Radio type`、`身份验证`/`Authentication`），只认一种会在另一种
/// 语言的系统上静默扫出空列表，用户还以为是网卡坏了。
///
/// 解析规则（与 Python 版 scan_networks 一致）：
/// - 条目以 BSSID 行为单位：一个 SSID 可带多个 BSSID（双频合一的天线各报一条）；
/// - 认证行属于它所在的网络：出现在 BSSID 之前时记入 `net_auth`（BSSID 行建条目时带上），
///   出现在之后时回填当前条目——两种顺序真实输出里都有；
/// - SSID 行值为空 → 隐藏网络，落 `(隐藏)` 兜底；
/// - 切分只认**第一个**冒号：SSID 名里可能含冒号，从最后一个切会截断。
pub fn parse_networks(text: &str) -> Vec<ScannedNetwork> {
    let mut aps: Vec<ScannedNetwork> = Vec::new();
    let mut cur_ssid = String::new();
    let mut net_auth = String::new();
    for line in text.lines() {
        let Some((label_raw, value)) = line.split_once([':', '：']) else {
            continue;
        };
        let label = label_raw.trim();
        let value = value.trim();
        // BSSID 行：建新条目。先于 SSID 判定只是防御——"BSSID" 并不以 "SSID" 开头
        if label.starts_with("BSSID") {
            aps.push(ScannedNetwork {
                ssid: if cur_ssid.is_empty() {
                    "(隐藏)".to_string()
                } else {
                    cur_ssid.clone()
                },
                bssid: value.to_string(),
                signal: 0,
                channel: 0,
                radio: String::new(),
                auth: net_auth.clone(),
                // 频段在收尾循环统一按信道号推导
                band: String::new(),
            });
            continue;
        }
        if label.starts_with("SSID") {
            // `SSID 1 :`（带索引，真实输出）与 `SSID :`（旧/异常格式）都认；
            // 值为空即隐藏网络，net_auth 随新网络重置
            cur_ssid = value.to_string();
            net_auth.clear();
            continue;
        }
        // 认证行独立处理：它可能出现在任何 BSSID 之前（此时还没有条目可回填，
        // 只能先记 net_auth）——放在 last_mut 守卫之前，否则会把它整个丢掉
        if label == "身份验证" || label == "Authentication" {
            net_auth = value.to_string();
            if let Some(cur) = aps.last_mut() {
                if cur.auth.is_empty() {
                    cur.auth = net_auth.clone();
                }
            }
            continue;
        }
        // 其余字段属于最近一个 BSSID 条目；条目还没出现时丢弃（与 Python 的
        // `isinstance(cur, dict)` 守卫同语义）
        let Some(cur) = aps.last_mut() else {
            continue;
        };
        if label == "信号" || label == "Signal" {
            // `82%` / `82` 都认：取值里的前导数字
            let digits: String = value
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(v) = digits.parse() {
                cur.signal = v;
            }
        } else if label == "信道" || label == "Channel" {
            let digits: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(v) = digits.parse() {
                cur.channel = v;
            }
        } else if label == "无线电类型" || label == "Radio type" {
            cur.radio = value.to_string();
        }
    }
    for ap in &mut aps {
        ap.band = band_of_channel(ap.channel).to_string();
    }
    // 信号强的排前面（与 Python 一致）；sort_by_key 稳定，同信号保持 netsh 原序
    aps.sort_by_key(|a| std::cmp::Reverse(a.signal));
    aps
}

/// 扫描周边 WiFi（BSSID 级）。
///
/// 为什么走 netsh 而不是 wlanapi 的 `WlanGetNetworkBssList`：那套 FFI 要手解
/// `WLAN_BSS_LIST` 的变长偏移数组，而扫描是低频用户动作（点按钮/10 秒 tick），
/// 毫秒级子进程开销可接受——与配置文件读取同一条 `run_capture` 通道，
/// 超时/不弹窗/PATH 解析行为完全一致。
pub fn scan_networks() -> Result<Vec<ScannedNetwork>, String> {
    let args: Vec<String> = ["netsh", "wlan", "show", "networks", "mode=bssid"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let out = crate::w32::run_capture(&args, 20).map_err(|e| format!("netsh 无法执行: {e}"))?;
    let text = out.combined();
    // Win11 出于隐私要求：位置服务关闭时 netsh 拒绝枚举网络。给可操作指引
    // 而不是"扫描失败"——用户自己能修，就不用提工单了
    if !out.ok && (text.contains("位置") || text.to_lowercase().contains("location")) {
        return Err("需要开启系统「位置」服务才能扫描 WiFi（设置→隐私和安全性→位置）".to_string());
    }
    Ok(parse_networks(&text))
}

// ---------------- WiFi 二维码分享 ----------------

/// WIFI QR 字段转义：`\` `;` `,` `:` `"` 五个字符前加反斜杠。
///
/// 这是 WIFI: URI 规范的字段分隔符集合，不转义会被当成结构符截断字段
/// （密码里带分号时二维码直接失效）。调用方无需保证顺序——逐字符处理天然
/// 先处理了反斜杠自身。
pub fn escape_wifi_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | ';' | ',' | ':' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// WIFI QR 的 T 字段取值（与 Python `_qr_ready` 的映射同口径）。
///
/// 判定按"包含"而不是全等：netsh 原文有 `WPA2-Personal` / `WPA2 - 个人` /
/// `WPA3-SAE` 等多种写法，逐个枚举迟早漏。无 WPA/WEP 字样的（开放/未知）一律
/// nopass——与 Python 一致；未知类型硬给 WPA 会生成连不上的二维码，更糟。
pub fn qr_auth_type(auth: &str) -> &'static str {
    if auth.is_empty() {
        return "nopass";
    }
    let a = auth.to_uppercase();
    if a.contains("WEP") {
        "WEP"
    } else if a.contains("WPA3") || a.contains("SAE") {
        "SAE"
    } else if a.contains("WPA") {
        // WPA2 也落这里：T:WPA 是 WIFI URI 里 WPA 家族的通用写法
        "WPA"
    } else {
        "nopass"
    }
}

/// 组装标准 WiFi 二维码字符串：`WIFI:T:<type>;S:<ssid>;P:<password>;;`。
///
/// nopass 按规范省略 P 字段（带着空 P 部分扫码器会当密码为空处理，行为不一）。
pub fn wifi_qr_string(ssid: &str, password: &str, auth: &str) -> String {
    let t = qr_auth_type(auth);
    let mut s = format!("WIFI:T:{t};S:{};", escape_wifi_field(ssid));
    if t != "nopass" {
        s.push_str(&format!("P:{};", escape_wifi_field(password)));
    }
    s.push(';');
    s
}

/// 二维码 SVG 渲染（`sk_clip_qr_svg` 的同款参数：纠错 M、最小 280）。
///
/// 抽出来是为了让 WiFi 命令复用同一条本地渲染路径，而不是复制三行 qrcode 调用。
pub fn render_qr_svg(data: &str) -> Result<String, String> {
    use qrcode::{render::svg, EcLevel, QrCode};
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::M)
        .map_err(|e| format!("二维码生成失败: {e}"))?;
    Ok(code.render::<svg::Color>().min_dimensions(280, 280).build())
}

#[cfg(test)]
mod credential_tests {
    use super::*;

    #[test]
    fn parses_chinese_netsh_output() {
        let out = "\
配置文件 SSID 的信息
-----------------------------------------------------------------------
    名称                   : MyWiFi
    控制选项               : 所有用户配置文件
    身份验证               : WPA2 - 个人
    关键内容               : my-secret-pass
";
        let (auth, pwd) = parse_profile_credential(out);
        assert_eq!(auth, "WPA2 - 个人");
        assert_eq!(pwd, "my-secret-pass");
    }

    #[test]
    fn parses_english_netsh_output() {
        // 系统语言为英文时输出不同——只认中文会静默返回空密码
        let out = "\
Profile MyWiFi on interface Wi-Fi:
-----------------------------------------------------------------------
    Applied: All User Profile
    Authentication         : WPA2-Personal
    Key Content            : my-secret-pass
";
        let (auth, pwd) = parse_profile_credential(out);
        assert_eq!(auth, "WPA2-Personal");
        assert_eq!(pwd, "my-secret-pass");
    }

    #[test]
    fn open_network_has_no_password_field() {
        let out = "    身份验证               : 开放式\n";
        let (auth, pwd) = parse_profile_credential(out);
        assert_eq!(auth, "开放式");
        assert_eq!(pwd, "", "开放网络本就没有密钥");
    }

    #[test]
    fn full_width_colon_is_accepted() {
        let out = "    关键内容：test123\n";
        let (_, pwd) = parse_profile_credential(out);
        assert_eq!(pwd, "test123");
    }

    #[test]
    fn password_with_colon_is_kept_whole() {
        // 密码含冒号：只能按**第一个**冒号切分，否则被截断成一半
        let out = "    关键内容               : ab:cd:ef\n";
        let (_, pwd) = parse_profile_credential(out);
        assert_eq!(pwd, "ab:cd:ef");
    }

    #[test]
    fn password_with_inner_spaces_is_preserved() {
        // 首尾空白剥掉，中间空格保留（密码里的空格是有效字符）
        let out = "    Key Content            :   my pass  \n";
        let (_, pwd) = parse_profile_credential(out);
        assert_eq!(pwd, "my pass");
    }

    #[test]
    fn missing_fields_yield_empty_strings() {
        let (auth, pwd) = parse_profile_credential("完全无关的输出\n");
        assert!(auth.is_empty() && pwd.is_empty());
    }

    #[test]
    fn first_occurrence_wins() {
        // 不能被子段里的空值覆盖
        let out = "    关键内容               : real\n    关键内容               : \n";
        let (_, pwd) = parse_profile_credential(out);
        assert_eq!(pwd, "real");
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    /// 固定的中文 netsh 样例（Win11 `wlan show networks mode=bssid` 真实版式）：
    /// 双 BSSID 合一 SSID、隐藏网络、中文名、2.4G 与 5G 混排。
    const SAMPLE_ZH: &str = "\
接口名称 : WLAN
有 4 个网络当前可见。

SSID 1 : MyHome
    网络类型               : 结构
    身份验证               : WPA2-Personal
    加密                   : CCMP
    BSSID 1                : aa:bb:cc:dd:ee:01
         信号                 : 82%
         无线电类型           : 802.11ax
         信道                 : 6
    BSSID 2                : aa:bb:cc:dd:ee:02
         信号                 : 40%
         无线电类型           : 802.11ax
         信道                 : 149

SSID 2 : Office-5G
    网络类型               : 结构
    身份验证               : WPA3-SAE
    加密                   : CCMP
    BSSID 1                : bb:cc:dd:ee:ff:03
         信号                 : 60%
         无线电类型           : 802.11ax
         信道                 : 44

SSID 3 :
    网络类型               : 结构
    身份验证               : 开放式
    加密                   : 无
    BSSID 1                : cc:dd:ee:ff:00:11
         信号                 : 30%
         无线电类型           : 802.11n
         信道                 : 1

SSID 4 : 前台WiFi
    网络类型               : 结构
    身份验证               : WPA2 - 个人
    加密                   : CCMP
    BSSID 1                : dd:ee:ff:00:11:22
         信号                 : 55%
         无线电类型           : 802.11ac
         信道                 : 11
";

    /// 固定的英文 netsh 样例（英文系统同版式）：字段标签全部换英文。
    const SAMPLE_EN: &str = "\
Interface name : WLAN
There are 4 networks currently visible.

SSID 1 : MyHome
    Network type            : Infrastructure
    Authentication          : WPA2-Personal
    Encryption              : CCMP
    BSSID 1                 : aa:bb:cc:dd:ee:01
         Signal             : 82%
         Radio type         : 802.11ax
         Channel            : 6
    BSSID 2                 : aa:bb:cc:dd:ee:02
         Signal             : 40%
         Radio type         : 802.11ax
         Channel            : 149

SSID 2 : Office-5G
    Network type            : Infrastructure
    Authentication          : WPA3-SAE
    Encryption              : CCMP
    BSSID 1                 : bb:cc:dd:ee:ff:03
         Signal             : 60%
         Radio type         : 802.11ax
         Channel            : 44

SSID 3 :
    Network type            : Infrastructure
    Authentication          : Open
    Encryption              : None
    BSSID 1                 : cc:dd:ee:ff:00:11
         Signal             : 30%
         Radio type         : 802.11n
         Channel            : 1

SSID 4 : Guest-WiFi
    Network type            : Infrastructure
    Authentication          : WPA2-Personal
    Encryption              : CCMP
    BSSID 1                 : dd:ee:ff:00:11:22
         Signal             : 55%
         Radio type         : 802.11ac
         Channel            : 11
";

    #[test]
    fn band_of_channel_matches_python_mapping() {
        assert_eq!(band_of_channel(0), "?");
        assert_eq!(band_of_channel(1), "2.4G");
        assert_eq!(band_of_channel(6), "2.4G");
        assert_eq!(band_of_channel(13), "2.4G");
        assert_eq!(
            band_of_channel(14),
            "2.4G",
            "信道 14（日本 legacy）仍属 2.4G"
        );
        assert_eq!(band_of_channel(15), "5G");
        assert_eq!(band_of_channel(36), "5G");
        assert_eq!(band_of_channel(165), "5G");
    }

    #[test]
    fn parses_chinese_sample_with_multi_bssid_and_hidden() {
        let aps = parse_networks(SAMPLE_ZH);
        // 4 个 SSID、5 个 BSSID：MyHome 双频各一条
        assert_eq!(aps.len(), 5, "条目按 BSSID 计数，不是按 SSID");
        // 信号降序：82 / 60 / 55 / 40 / 30
        let signals: Vec<u32> = aps.iter().map(|a| a.signal).collect();
        assert_eq!(signals, vec![82, 60, 55, 40, 30]);

        let home0 = &aps[0];
        assert_eq!(home0.ssid, "MyHome");
        assert_eq!(home0.bssid, "aa:bb:cc:dd:ee:01");
        assert_eq!(home0.channel, 6);
        assert_eq!(home0.radio, "802.11ax");
        assert_eq!(home0.auth, "WPA2-Personal");
        assert_eq!(home0.band, "2.4G");

        // 同 SSID 的第二个 BSSID（5G）：认证/SSID 不串行
        let home1 = &aps[3];
        assert_eq!(home1.ssid, "MyHome");
        assert_eq!(home1.bssid, "aa:bb:cc:dd:ee:02");
        assert_eq!(home1.channel, 149);
        assert_eq!(home1.band, "5G");
        assert_eq!(home1.auth, "WPA2-Personal");

        assert_eq!(aps[1].auth, "WPA3-SAE");
        // 隐藏网络：SSID 行值为空 → (隐藏) 兜底
        assert_eq!(aps[4].ssid, "(隐藏)");
        assert_eq!(aps[4].bssid, "cc:dd:ee:ff:00:11");
        // 中文名原样保留
        assert_eq!(aps[2].ssid, "前台WiFi");
        assert_eq!(aps[2].auth, "WPA2 - 个人");
    }

    #[test]
    fn parses_english_sample_identically() {
        // 中英样例除语言外逐字段同构：解析结果必须一致（前台WiFi ↔ Guest-WiFi 是唯一名差异）
        let zh = parse_networks(SAMPLE_ZH);
        let en = parse_networks(SAMPLE_EN);
        assert_eq!(zh.len(), en.len());
        for (z, e) in zh.iter().zip(en.iter()) {
            assert_eq!(z.bssid, e.bssid);
            assert_eq!(z.signal, e.signal);
            assert_eq!(z.channel, e.channel);
            assert_eq!(z.radio, e.radio, "无线电类型值本身不随语言变化");
            assert_eq!(
                z.auth
                    .replace("WPA2 - 个人", "WPA2-Personal")
                    .replace("开放式", "Open"),
                e.auth,
                "认证值仅语言差异"
            );
        }
        assert_eq!(en[4].ssid, "(隐藏)");
    }

    #[test]
    fn auth_line_after_bssid_backfills_current_entry() {
        // 旧版 netsh 把认证行放在 BSSID 块之后：也要能归到当前条目
        let out = "\
SSID 1 : Legacy
    BSSID 1                : 11:22:33:44:55:66
         信号                 : 45%
         信道                 : 11
    身份验证               : WPA2-Personal
";
        let aps = parse_networks(out);
        assert_eq!(aps.len(), 1);
        assert_eq!(aps[0].auth, "WPA2-Personal");
        assert_eq!(aps[0].channel, 11);
        assert_eq!(aps[0].band, "2.4G");
    }

    #[test]
    fn auth_line_before_any_bssid_is_kept_via_net_auth() {
        // 认证行出现在任何 BSSID 之前时只进 net_auth（没有条目可回填），
        // BSSID 行创建条目时必须带上——放在 last_mut 守卫之后就会丢掉它
        let out = "\
SSID 1 : BeforeAuth
    身份验证               : WPA2-Personal
    BSSID 1                : 11:22:33:44:55:66
";
        let aps = parse_networks(out);
        assert_eq!(aps[0].auth, "WPA2-Personal");
    }

    #[test]
    fn auth_resets_between_networks() {
        // 前一个网络的认证不得漏给下一个（net_auth 在 SSID 行重置）
        let out = "\
SSID 1 : A
    身份验证               : WPA2-Personal
    BSSID 1                : aa:aa:aa:aa:aa:aa

SSID 2 : B
    身份验证               : 开放式
    BSSID 1                : bb:bb:bb:bb:bb:bb
";
        let aps = parse_networks(out);
        assert_eq!(aps[0].auth, "WPA2-Personal");
        assert_eq!(aps[1].auth, "开放式");
    }

    #[test]
    fn signal_without_percent_and_full_width_colon() {
        // 全角冒号 + 无百分号的信号值（异常/旧格式容错）
        let out = "SSID 1 : X\n    BSSID 1 : 11:22:33:44:55:66\n         信号：75\n";
        let aps = parse_networks(out);
        assert_eq!(aps[0].signal, 75);
    }

    #[test]
    fn missing_fields_stay_default_not_fabricated() {
        // 只有 SSID/BSSID 时：signal=0 / channel=0 / band=? ——不编造读数
        let out = "SSID 1 : Bare\n    BSSID 1 : 11:22:33:44:55:66\n";
        let aps = parse_networks(out);
        assert_eq!(aps[0].signal, 0);
        assert_eq!(aps[0].channel, 0);
        assert_eq!(aps[0].band, "?");
        assert_eq!(aps[0].radio, "");
    }

    #[test]
    fn empty_and_garbage_inputs_yield_empty_list() {
        assert!(parse_networks("").is_empty());
        assert!(parse_networks("完全无关的输出\nno colon lines\n").is_empty());
        // 只有 SSID 行没有 BSSID 行：不产生条目（条目以 BSSID 为单位）
        assert!(parse_networks("SSID 1 : OnlySsid\n").is_empty());
    }

    #[test]
    fn ssid_with_colon_is_kept_whole() {
        // SSID 名含冒号：只按第一个冒号切分，否则名字被截断
        let out = "SSID 1 : My:Net\n    BSSID 1 : 11:22:33:44:55:66\n";
        let aps = parse_networks(out);
        assert_eq!(aps[0].ssid, "My:Net");
    }

    #[test]
    fn interface_header_lines_do_not_pollute_entries() {
        // 接口级字段（接口名称/状态/配置文件）不属于任何 BSSID 条目
        let out = "\
接口名称 : WLAN
配置文件 : MyHome
SSID 1 : MyHome
    BSSID 1 : 11:22:33:44:55:66
         信号 : 70%
         信道 : 36
";
        let aps = parse_networks(out);
        assert_eq!(aps.len(), 1);
        assert_eq!(aps[0].channel, 36);
        assert_eq!(aps[0].band, "5G");
    }
}

#[cfg(test)]
mod wifi_qr_tests {
    use super::*;

    #[test]
    fn escapes_all_five_special_chars() {
        // WIFI URI 的字段分隔符集合：\ ; , : " ——少转义一个，字段就被截断
        assert_eq!(escape_wifi_field("a;b"), "a\\;b");
        assert_eq!(escape_wifi_field("a,b"), "a\\,b");
        assert_eq!(escape_wifi_field("a:b"), "a\\:b");
        assert_eq!(escape_wifi_field("a\"b"), "a\\\"b");
        assert_eq!(escape_wifi_field("a\\b"), "a\\\\b", "反斜杠自身必须先转义");
        assert_eq!(escape_wifi_field("plain"), "plain");
        assert_eq!(escape_wifi_field(""), "");
        // 中文等非特殊字符原样保留
        assert_eq!(escape_wifi_field("前台WiFi"), "前台WiFi");
    }

    #[test]
    fn escapes_combined_hostile_password() {
        let s = "p\\a;ss:,wo\"rd";
        let expected = "p\\\\a\\;ss\\:\\,wo\\\"rd";
        assert_eq!(escape_wifi_field(s), expected);
    }

    #[test]
    fn qr_auth_type_maps_netsh_writings() {
        // 中英 netsh 原文都要能映射；判定按"包含"，逐个枚举写法迟早漏
        assert_eq!(qr_auth_type("WPA2-Personal"), "WPA");
        assert_eq!(qr_auth_type("WPA2 - 个人"), "WPA");
        assert_eq!(qr_auth_type("WPA-Personal"), "WPA");
        assert_eq!(qr_auth_type("WPA3-SAE"), "SAE");
        assert_eq!(qr_auth_type("SAE"), "SAE");
        assert_eq!(qr_auth_type("WEP"), "WEP");
        assert_eq!(qr_auth_type("开放式"), "nopass");
        assert_eq!(qr_auth_type("Open"), "nopass");
        assert_eq!(qr_auth_type(""), "nopass");
        // 未知类型按 nopass（与 Python 一致）：硬给 WPA 会生成连不上的二维码
        assert_eq!(qr_auth_type("Unknown"), "nopass");
    }

    #[test]
    fn wifi_qr_string_standard_shape() {
        assert_eq!(
            wifi_qr_string("MyNet", "pass", "WPA2-Personal"),
            "WIFI:T:WPA;S:MyNet;P:pass;;"
        );
        // nopass 省略 P 字段（WIFI URI 规范）
        assert_eq!(
            wifi_qr_string("Cafe", "", "开放式"),
            "WIFI:T:nopass;S:Cafe;;"
        );
        assert_eq!(wifi_qr_string("Cafe", "", ""), "WIFI:T:nopass;S:Cafe;;");
        // WPA3 走 SAE
        assert_eq!(wifi_qr_string("n", "p", "WPA3-SAE"), "WIFI:T:SAE;S:n;P:p;;");
    }

    #[test]
    fn wifi_qr_string_escapes_ssid_and_password() {
        assert_eq!(
            wifi_qr_string("a;b", "c:d", "WPA"),
            "WIFI:T:WPA;S:a\\;b;P:c\\:d;;"
        );
    }

    #[test]
    fn render_qr_svg_produces_local_svg() {
        let svg = render_qr_svg(&wifi_qr_string("MyNet", "pass", "WPA2-Personal"))
            .expect("常规 WIFI 串必须可渲染");
        assert!(
            svg.starts_with("<?xml") || svg.starts_with("<svg"),
            "必须是 SVG 文本：{svg}"
        );
        assert!(svg.contains("<svg"), "缺 svg 根元素");
        assert!(svg.contains("viewBox"), "SVG 要带 viewBox 以便缩放");
        // 同一输入渲染稳定（后端本地生成，没有随机性）
        let again = render_qr_svg(&wifi_qr_string("MyNet", "pass", "WPA2-Personal")).unwrap();
        assert_eq!(svg, again);
    }

    #[test]
    fn render_qr_svg_rejects_oversized_payload() {
        // 与 sk_clip_qr_svg 的容量守卫同口径：超容量输入必须 Err 而不是 panic
        let big = "x".repeat(4000);
        assert!(render_qr_svg(&big).is_err());
    }
}
