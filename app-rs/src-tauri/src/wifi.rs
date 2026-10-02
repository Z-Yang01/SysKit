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
