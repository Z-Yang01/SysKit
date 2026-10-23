//! 已配对蓝牙设备与电量。
//!
//! 数据来源两层（能力边界写在最前面，避免以后误以为是"全套 API"）：
//!
//! 1. **设备清单**：注册表 `HKLM\SYSTEM\CurrentControlSet\Services\BTHPORT\
//!    Parameters\Devices\<MAC>`——配对过就在，与是否连接无关。
//!    注意 `Name` 是 **REG_BINARY 的 UTF-8**（不是 REG_SZ），按字符串读会得到乱码。
//! 2. **电量**：WinRT BLE GATT 电池服务（服务 0x180F / 特征 0x2A19）。
//!    Windows 没有公开的「蓝牙设备电量」Win32 API，只有这条 GATT 路；
//!    **读不到是常态**——经典蓝牙耳机、部分鼠标根本不广播电量，
//!    所以调用方必须能显示"未上报"，而不是把失败当错误。
//!
//! GATT 读取会阻塞到设备响应，未连接的设备可能要等好几秒：本模块内部
//! 用线程 + 超时兜底（默认 4 秒），保证界面不会被一个失联设备拖住。

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;

/// 配对设备（不含电量；电量按需单独读，避免列表刷新被 GATT 拖慢）。
#[derive(Debug, Clone, Serialize)]
pub struct PairedDevice {
    /// 规范化 MAC：`AA:BB:CC:DD:EE:FF`（注册表子键名是 12 位无分隔十六进制）
    pub addr: String,
    pub name: String,
    /// 设备类别（由 Class of Device 推断，缺 Class 时为「蓝牙设备」）
    pub kind: String,
}

// **必须带 HKLM\ 前缀**：w32::split_reg_path 按第一个反斜杠切根键，
// 少了前缀这里会被当成未知根键 → 静默返回空表（v1.9.0 的 bug 就出在这）
const BTHPORT_DEVICES: &str =
    r"HKLM\SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters\Devices";

/// CoD 里的 Major Device Class（位 8..12）。
///
/// 只做粗分类：用户要的是"这是个耳机还是鼠标"，蓝牙 SIG 的细分（minor class）
/// 对判断毫无帮助，反而增加误判面。
pub fn cod_kind(cod: u32) -> &'static str {
    match (cod >> 8) & 0x1f {
        0x01 => "电脑",
        0x02 => "手机",
        0x03 => "网络设备",
        0x04 => "音频设备",
        0x05 => "外设（鼠标/键盘）",
        0x06 => "影像设备",
        0x07 => "穿戴设备",
        0x08 => "玩具",
        0x09 => "健康设备",
        _ => "蓝牙设备",
    }
}

/// 12 位十六进制 → `AA:BB:CC:DD:EE:FF`；非 12 位原样返回（不猜）。
fn normalize_addr(raw: &str) -> String {
    let s = raw.trim();
    if s.len() != 12 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return s.to_string();
    }
    s.as_bytes()
        .chunks(2)
        .map(|c| String::from_utf8_lossy(c).to_uppercase())
        .collect::<Vec<_>>()
        .join(":")
}

/// `AA:BB:CC:DD:EE:FF` → `BLUETOOTH_ADDRESS.ullLong`。
///
/// 注意**字节序**：Win32 的 BLUETOOTH_ADDRESS 低位存 MAC 的第一个字节，
/// 所以 `AA:BB:CC:DD:EE:FF` 要写成 `0x0000_FFEE_DDCC_BBAA`（与 WinRT 的
/// 48 位整数相反）。搞反了会去删另一台设备——这是本函数独立测试的原因。
pub fn addr_to_bytes(addr: &str) -> Option<[u8; 6]> {
    let hex: String = addr.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 12 {
        return None;
    }
    let mut b = [0u8; 6];
    for (i, slot) in b.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(b)
}

/// 移除配对（Win32 `BluetoothRemoveDevice`）。
///
/// 只删配对关系，不碰用户数据；设备下次靠近需重新配对——所以界面必须二次确认。
pub fn unpair(addr: &str) -> Result<(), String> {
    use windows::Win32::Devices::Bluetooth::{
        BluetoothRemoveDevice, BLUETOOTH_ADDRESS, BLUETOOTH_ADDRESS_0,
    };
    // 用 rgBytes（MAC 正序）而不是 ullLong：后者是低位在前，写反会删错设备
    let bytes = addr_to_bytes(addr).ok_or_else(|| format!("地址不合法: {addr}"))?;
    let a = BLUETOOTH_ADDRESS {
        Anonymous: BLUETOOTH_ADDRESS_0 { rgBytes: bytes },
    };
    let rc = unsafe { BluetoothRemoveDevice(&a) };
    if rc == 0 {
        Ok(())
    } else {
        Err(format!("移除配对失败（错误码 {rc}，可能需要管理员权限）"))
    }
}

/// 开关蓝牙无线电（`Radio::SetStateAsync`），返回**实际生效的状态**。
///
/// 比"打开设置页让用户自己点"少一次跳转；被组策略/用户拒绝时返回 Err 说明原因。
pub fn set_radio(on: bool) -> Result<bool, String> {
    use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};

    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(
            windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
        );
    }
    let list = block_on(Radio::GetRadiosAsync().map_err(|e| format!("枚举无线电失败: {e}"))?)
        .map_err(|e| format!("等待无线电列表失败: {e}"))?;
    let want = if on { RadioState::On } else { RadioState::Off };
    for r in list {
        if !matches!(r.Kind(), Ok(RadioKind::Bluetooth)) {
            continue;
        }
        let st = block_on(
            r.SetStateAsync(want)
                .map_err(|e| format!("设置蓝牙状态失败: {e}"))?,
        )
        .map_err(|e| format!("等待状态切换失败: {e}"))?;
        return match st {
            RadioAccessStatus::Allowed => Ok(on),
            RadioAccessStatus::DeniedByUser => Err("系统拒绝了这次切换（可能被策略或用户设置锁定）".into()),
            _ => Err("蓝牙状态切换未被允许".into()),
        };
    }
    Err("本机没有蓝牙适配器".into())
}

/// `AA:BB:CC:DD:EE:FF` → u64（WinRT 需要 48 位地址）。
pub fn addr_to_u64(addr: &str) -> Option<u64> {
    let hex: String = addr.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 12 {
        return None;
    }
    u64::from_str_radix(&hex, 16).ok()
}

/// 注册表值 → 设备名：兼容 REG_BINARY(UTF-8) 与 REG_SZ 两种历史写法。
fn decode_name(v: &crate::w32::RegValue) -> String {
    use crate::w32::RegValue;
    let s = match v {
        RegValue::Str(s) => s.clone(),
        RegValue::Binary(b) => {
            let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
            String::from_utf8_lossy(&b[..end]).to_string()
        }
        _ => String::new(),
    };
    s.trim_matches('\0').trim().to_string()
}

/// 枚举已配对设备（注册表，毫秒级）。
pub fn list_paired() -> Vec<PairedDevice> {
    let mut out = Vec::new();
    for (mac, values) in crate::w32::reg_subkeys(BTHPORT_DEVICES) {
        let name = values
            .get("Name")
            .map(decode_name)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("未知设备 {}", normalize_addr(&mac)));
        let kind = match values.get("Class") {
            Some(crate::w32::RegValue::Dword(cod)) => cod_kind(*cod).to_string(),
            _ => "蓝牙设备".to_string(),
        };
        out.push(PairedDevice {
            addr: normalize_addr(&mac),
            name,
            kind,
        });
    }
    // 名字排序：注册表顺序是随机的，每次刷新顺序跳动会让人以为设备在变
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// 蓝牙无线电状态：`None` = 本机没有蓝牙适配器。
///
/// 有它才能区分"蓝牙没开"和"设备没连"——两者在设备列表里都表现为探测失败，
/// 但用户该做的事完全不同（开蓝牙 vs 走近/开机）。
pub fn radio_state() -> Option<bool> {
    use windows::Devices::Radios::{Radio, RadioKind, RadioState};

    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(
            windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
        );
    }
    let list = block_on(Radio::GetRadiosAsync().ok()?).ok()?;
    for r in list {
        // 只看蓝牙那一个无线电（机器可能还有 WiFi 无线电）
        if matches!(r.Kind(), Ok(RadioKind::Bluetooth)) {
            return Some(matches!(r.State(), Ok(RadioState::On)));
        }
    }
    None
}

/// 单个设备的探测结果：是否已连接 + 电量（没有就 None）。
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Probe {
    pub connected: bool,
    pub battery: Option<u8>,
}

/// 批量探测（并行，每个设备独立超时）。
pub fn probes(addrs: &[String], timeout: Duration) -> BTreeMap<String, Option<Probe>> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = addrs
            .iter()
            .map(|a| {
                let addr = a.clone();
                scope.spawn(move || {
                    let v = with_timeout(timeout, move || read_probe(&addr));
                    (a.clone(), v.ok().flatten())
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .collect::<BTreeMap<String, Option<Probe>>>()
    })
}

/// 在独立线程里跑 + 超时：WinRT 的 `.get()` 对失联设备可能长时间不返回，
/// 直接在主命令线程上等会把「设备页刷新」拖成十几秒。
fn with_timeout<F>(timeout: Duration, f: F) -> Result<Option<Probe>, String>
where
    F: FnOnce() -> Result<Option<Probe>, String> + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(_) => Err("设备未响应（可能未连接）".into()),
    }
}

/// 读单个设备的电量（GATT 0x180F/0x2A19）。
///
/// 返回 `Ok(None)` = 设备存在但没上报电量（不支持/未连接）；
/// `Err` = 这次读取本身失败（超时/权限）。界面两者都显示「—」，
/// 但排障时能区分是"设备不支持"还是"这次没读到"。
pub fn read_probe(addr: &str) -> Result<Option<Probe>, String> {
    let Some(num) = addr_to_u64(addr) else {
        return Err(format!("地址不合法: {addr}"));
    };
    use windows::Devices::Bluetooth::BluetoothLEDevice;
    use windows::Devices::Bluetooth::GenericAttributeProfile::{
        GattCharacteristic, GattDeviceService,
    };
    use windows::Storage::Streams::DataReader;

    // 电池服务 / 电池电量特征（蓝牙 SIG 分配号）
    const BATTERY_SERVICE: u128 = 0x0000_180F_0000_1000_8000_0080_5F9B_34FB;
    const BATTERY_LEVEL: u128 = 0x0000_2A19_0000_1000_8000_0080_5F9B_34FB;

    // 每个工作线程都要 COM 初始化；已初始化时返回 RPC_E_CHANGED_MODE，忽略即可
    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(
            windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
        );
    }

    dbg(&format!("{addr} 创建句柄"));
    let dev = match block_on(
        BluetoothLEDevice::FromBluetoothAddressAsync(num)
            .map_err(|e| format!("创建蓝牙设备句柄失败: {e}"))?,
    ) {
        Ok(d) => d,
        // **空结果不是错误**：设备不在范围内/未缓存时，WinRT 返回成功但结果为
        // null，windows crate 会包成 HRESULT 0（"操作成功完成"）的 Err。
        // 按"设备不可达"处理——语义就是未连接，不该让 7/9 台设备全变成"离线"。
        Err(e) if e.code().0 == 0 => {
            dbg(&format!("{addr} 不可达（空结果）"));
            return Ok(Some(Probe { connected: false, battery: None }));
        }
        Err(e) => return Err(format!("连接蓝牙设备失败: {e}")),
    };
    dbg(&format!("{addr} 句柄就绪，读连接状态"));

    // 连接状态先看：未连接的设备读 GATT 只会白等到超时
    let connected = matches!(
        dev.ConnectionStatus(),
        Ok(windows::Devices::Bluetooth::BluetoothConnectionStatus::Connected)
    );
    dbg(&format!("{addr} connected={connected}"));
    if !connected {
        return Ok(Some(Probe { connected: false, battery: None }));
    }

    dbg(&format!("{addr} 枚举 GATT 服务"));
    let services = block_on(
        dev.GetGattServicesAsync()
            .map_err(|e| format!("枚举 GATT 服务失败: {e}"))?,
    )
    .map_err(|e| format!("等待 GATT 服务失败: {e}"))?
    .Services()
    .map_err(|e| format!("读取服务列表失败: {e}"))?;

    for svc in services {
        let svc: GattDeviceService = svc;
        let Ok(uuid) = svc.Uuid() else { continue };
        if uuid != windows::core::GUID::from_u128(BATTERY_SERVICE) {
            continue;
        }
        let chars = block_on(
            svc.GetCharacteristicsAsync()
                .map_err(|e| format!("枚举特征失败: {e}"))?,
        )
        .map_err(|e| format!("等待特征失败: {e}"))?
        .Characteristics()
        .map_err(|e| format!("读取特征列表失败: {e}"))?;
        for ch in chars {
            let ch: GattCharacteristic = ch;
            let Ok(uuid) = ch.Uuid() else { continue };
            if uuid != windows::core::GUID::from_u128(BATTERY_LEVEL) {
                continue;
            }
            let Ok(op) = ch.ReadValueAsync() else {
                continue;
            };
            let res = block_on(op).map_err(|e| format!("读取电量失败: {e}"))?;
            let buf = res.Value().map_err(|e| format!("取值失败: {e}"))?;
            let reader = DataReader::FromBuffer(&buf).map_err(|e| format!("解析失败: {e}"))?;
            let pct = reader.ReadByte().map_err(|e| format!("解析电量失败: {e}"))?;
            // 0..=100 之外的值按"未上报"处理，别把脏数据显示成电量
            return Ok(Some(Probe {
                connected: true,
                battery: (pct <= 100).then_some(pct),
            }));
        }
    }
    // 没找到电池服务 = 设备不上报（耳机/老鼠标常见），不是错误
    Ok(Some(Probe { connected: true, battery: None }))
}

/// 阶段诊断：仅在 `SK_DEV_DEBUG=1` 时输出（排查 WinRT 卡在哪一步用）。
fn dbg(msg: &str) {
    if std::env::var_os("SK_DEV_DEBUG").is_some() {
        eprintln!("[devices] {msg}");
    }
}

/// 最小的 future 阻塞器（windows-future 0.3 只实现 `IntoFuture`，没有 `get()`）。
///
/// 只服务本模块的"一次 WinRT 调用等结果"：WinRT 完成回调在别的线程触发
/// wake → unpark；park 前先 swap 标志，避免 wake 与 park 之间的丢失唤醒。
fn block_on<F>(fut: F) -> F::Output
where
    F: std::future::IntoFuture,
    F::IntoFuture: std::future::Future,
{
    use std::future::Future as _; // poll() 是 Future 的 trait 方法
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    /// **wake 必须 unpark**：只置标志的话，park 等待的线程永远醒不过来，
    /// 每个 WinRT 调用都会白白等到外层超时（v1.9.0 的真 bug——9 台设备
    /// 全部 4 秒超时，界面显示"未知"）。
    struct Flag {
        fired: AtomicBool,
        thread: std::thread::Thread,
    }
    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.fired.store(true, Ordering::SeqCst);
            self.thread.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.fired.store(true, Ordering::SeqCst);
            self.thread.unpark();
        }
    }

    let mut fut = std::pin::pin!(fut.into_future());
    let flag = Arc::new(Flag {
        fired: AtomicBool::new(false),
        thread: std::thread::current(),
    });
    let waker = Waker::from(flag.clone());
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => {
                while !flag.fired.swap(false, Ordering::SeqCst) {
                    std::thread::park();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cod_kind_maps_major_class() {
        // 0x05 << 8 = 外设（鼠标/键盘）
        assert_eq!(cod_kind(0x0000_0540), "外设（鼠标/键盘）");
        assert_eq!(cod_kind(0x0000_0404), "音频设备");
        assert_eq!(cod_kind(0x0000_010C), "电脑");
        assert_eq!(cod_kind(0), "蓝牙设备");
    }

    #[test]
    fn normalize_addr_formats_mac() {
        assert_eq!(normalize_addr("aabbccddeeff"), "AA:BB:CC:DD:EE:FF");
        // 非 12 位原样返回：不猜、不补零
        assert_eq!(normalize_addr("aabb"), "aabb");
    }

    /// 字节序测试：写反了会删错设备，这条必须锁住。
    #[test]
    fn addr_to_bytes_keeps_mac_order() {
        assert_eq!(addr_to_bytes("AA:BB:CC:DD:EE:FF"), Some([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]));
        assert_eq!(addr_to_bytes("00:11:22:33:44:55"), Some([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]));
        assert_eq!(addr_to_bytes("AA:BB"), None);
        // ullLong 视角的同一条地址（低位在前）：留给排障时对照用
        let b = addr_to_bytes("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(
            u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], 0, 0]),
            0x0000_FFEE_DDCC_BBAA
        );
    }

    #[test]
    fn addr_to_u64_round_trip() {
        assert_eq!(addr_to_u64("AA:BB:CC:DD:EE:FF"), Some(0xAABBCCDDEEFF));
        assert_eq!(addr_to_u64("AA:BB"), None);
    }

    /// v1.9.0 的真 bug：路径少了 `HKLM\` 前缀 → split_reg_path 认不出根键 →
    /// reg_subkeys 静默返回空表 → "已连接的蓝牙设备一个都不显示"。
    /// 这条测试锁住前缀，别再让静默空表回来。
    #[test]
    fn bthport_path_has_hive_prefix() {
        assert!(
            BTHPORT_DEVICES.starts_with("HKLM\\") || BTHPORT_DEVICES.starts_with("HKEY_LOCAL_MACHINE\\"),
            "注册表路径必须以根键开头，否则会被解析为空表"
        );
    }

    /// 真机冒烟（默认 ignore）：确认本机能枚举出已配对设备。
    /// 手工跑：cargo test list_paired_on_real_machine -- --ignored --nocapture
    #[test]
    #[ignore]
    fn list_paired_on_real_machine() {
        let devs = list_paired();
        println!("发现 {} 台已配对设备", devs.len());
        for d in devs.iter().take(10) {
            println!("  {} {} [{}]", d.addr, d.name, d.kind);
        }
        assert!(!devs.is_empty(), "本机应至少有一台已配对设备");
    }

    /// 真机冒烟（默认 ignore）：跑一遍 WinRT 探测，确认 COM 初始化 +
    /// 自写 block_on 在真实环境不崩、不挂死。手工跑：
    /// cargo test probe_on_real_machine -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe_on_real_machine() {
        let devs = list_paired();
        let addrs: Vec<String> = devs.iter().map(|d| d.addr.clone()).collect();
        let t0 = std::time::Instant::now();
        let m = probes(&addrs, Duration::from_secs(4));
        println!("探测 {} 台耗时 {:?}", addrs.len(), t0.elapsed());
        for d in &devs {
            println!("  {} → {:?}", d.name, m.get(&d.addr));
        }
    }

    /// 真机冒烟（默认 ignore）：**只做幂等开启**，绝不在这里关蓝牙——
    /// 关掉之后若开启失败，用户就断网/断设备了。手工跑：
    /// cargo test radio_power_on_idempotent -- --ignored --nocapture
    #[test]
    #[ignore]
    fn radio_power_on_idempotent() {
        let r = set_radio(true);
        println!("set_radio(true) = {r:?}");
        assert!(r.is_ok(), "开启蓝牙应当成功（幂等）");
    }

    /// 真机冒烟（默认 ignore）：蓝牙无线电状态能否读到。
    /// cargo test radio_state_on_real_machine -- --ignored --nocapture
    /// 串行 vs 并行对照（默认 ignore）：判断 7/9 台失败是"不在范围"还是
    /// "并发抢蓝牙栈"。cargo test probe_serial_on_real_machine -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe_serial_on_real_machine() {
        for d in list_paired() {
            let t0 = std::time::Instant::now();
            let r = read_probe(&d.addr);
            println!("  {:?} {} → {:?}（{:?}）", r.is_ok(), d.name, r, t0.elapsed());
        }
    }

    #[test]
    #[ignore]
    fn radio_state_on_real_machine() {
        let t0 = std::time::Instant::now();
        let st = radio_state();
        println!("radio_state = {st:?} 耗时 {:?}", t0.elapsed());
    }

    #[test]
    fn decode_name_reads_binary_utf8() {
        use crate::w32::RegValue;
        // 注册表里 Name 是 REG_BINARY 的 UTF-8（带 NUL 结尾）
        let mut bytes = "蓝牙耳机".as_bytes().to_vec();
        bytes.push(0);
        assert_eq!(decode_name(&RegValue::Binary(bytes)), "蓝牙耳机");
        assert_eq!(decode_name(&RegValue::Str("鼠标".into())), "鼠标");
    }
}
