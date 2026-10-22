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

const BTHPORT_DEVICES: &str =
    r"SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters\Devices";

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

/// 批量读电量（并行，每个设备独立超时）。
pub fn batteries(addrs: &[String], timeout: Duration) -> BTreeMap<String, Option<u8>> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = addrs
            .iter()
            .map(|a| {
                let addr = a.clone();
                scope.spawn(move || {
                    let v = with_timeout(timeout, move || read_battery(&addr));
                    (a.clone(), v.ok().flatten())
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .collect::<BTreeMap<String, Option<u8>>>()
    })
}

/// 在独立线程里跑 + 超时：WinRT 的 `.get()` 对失联设备可能长时间不返回，
/// 直接在主命令线程上等会把「设备页刷新」拖成十几秒。
fn with_timeout<F>(timeout: Duration, f: F) -> Result<Option<u8>, String>
where
    F: FnOnce() -> Result<Option<u8>, String> + Send + 'static,
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
pub fn read_battery(addr: &str) -> Result<Option<u8>, String> {
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

    let dev = block_on(
        BluetoothLEDevice::FromBluetoothAddressAsync(num)
            .map_err(|e| format!("创建蓝牙设备句柄失败: {e}"))?,
    )
    .map_err(|e| format!("连接蓝牙设备失败: {e}"))?;

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
            return Ok((pct <= 100).then_some(pct));
        }
    }
    // 没找到电池服务 = 设备不上报（耳机/老鼠标常见），不是错误
    Ok(None)
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

    struct Flag(AtomicBool);
    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let mut fut = std::pin::pin!(fut.into_future());
    let flag = Arc::new(Flag(AtomicBool::new(false)));
    let waker = Waker::from(flag.clone());
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => {
                while !flag.0.swap(false, Ordering::SeqCst) {
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

    #[test]
    fn addr_to_u64_round_trip() {
        assert_eq!(addr_to_u64("AA:BB:CC:DD:EE:FF"), Some(0xAABBCCDDEEFF));
        assert_eq!(addr_to_u64("AA:BB"), None);
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
