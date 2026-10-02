//! 遥测采样：1 秒网络速率 / CPU / 内存 / 磁盘（对应 Python 侧 telemetry.py）。
//!
//! 设计取舍（与 M1 的 w32 一样）：**Win32 调用与纯逻辑分离**。
//! 速率过滤、增量计算、CPU 百分比这些都提成纯函数，用固定输入断言；
//! 只有取数那一步碰 FFI。这样「物理网卡过滤」「负增量夹零」「CPU 基线」这些
//! 真正容易错的地方都能被单测钉住，而不是只能靠真机肉眼看曲线。
//!
//! Python 侧的网络速率来自 psutil `net_io_counters(pernic=True)`；这里用
//! `GetIfTable2`（同一份内核计数器的直接来源），并按**完全相同的 11 个关键字**
//! 过滤虚拟网卡——过滤表是跨版本行为契约，改它就会让两版的曲线分叉。

use std::collections::BTreeMap;

/// 虚拟/隧道网卡关键字（逐项对齐 Python telemetry._PHYSICAL_EXCLUDE）。
pub const NIC_EXCLUDE: [&str; 11] = [
    "loopback",
    "vmnet",
    "vmware",
    "vethernet",
    "bluetooth",
    "wan miniport",
    "tap",
    "wintun",
    "utun",
    "vgate",
    "hyper-v",
];

/// 单个网卡的字节计数器快照。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IfCounters {
    pub recv: u64,
    pub sent: u64,
}

/// 该网卡名是否算「物理」网卡（Python: `lname == "loopback" or any(x in lname ...)`）。
pub fn is_physical(name: &str) -> bool {
    let lname = name.to_lowercase();
    if lname == "loopback" {
        return false;
    }
    !NIC_EXCLUDE.iter().any(|x| lname.contains(x))
}

/// 由两次快照算 (down_Bps, up_Bps)：只看物理网卡，负增量夹到 0。
///
/// 与 Python 的差异仅在「用别名而非 psutil 名」：两边都是「名称 → 计数器」的
/// 映射，键一致即可。上一拍不存在的网卡跳过（对应 Python `if not p: continue`），
/// 这样新建的虚拟网卡不会把它的历史累计量当成一次瞬时暴增。
pub fn compute_rates(
    last: &BTreeMap<String, IfCounters>,
    cur: &BTreeMap<String, IfCounters>,
) -> (u64, u64) {
    let mut down = 0u64;
    let mut up = 0u64;
    for (name, c) in cur {
        if !is_physical(name) {
            continue;
        }
        let Some(p) = last.get(name) else {
            continue;
        };
        down += c.recv.saturating_sub(p.recv);
        up += c.sent.saturating_sub(p.sent);
    }
    (down, up)
}

/// CPU 负载百分比：由两次 `GetSystemTimes` 累计值求差。
///
/// 关系与 psutil 一致：`idle / (idle + kernel + user)`，其中 kernel 已含 idle。
/// 两次差全为 0（同一 tick 内重复采样）时返回 0.0，绝不除零。
pub fn cpu_percent(prev: (u64, u64, u64), cur: (u64, u64, u64)) -> f64 {
    let (pi, pk, pu) = prev;
    let (ci, ck, cu) = cur;
    let idle = ci.saturating_sub(pi);
    let kernel = ck.saturating_sub(pk);
    let user = cu.saturating_sub(pu);
    let total = kernel + user; // kernel 含 idle，故总忙+闲 = kernel + user
    if total == 0 {
        return 0.0;
    }
    let busy = total.saturating_sub(idle);
    (busy as f64) * 100.0 / (total as f64)
}

/// 人类可读字节数（对齐 Python charts.human_bytes 的口径）。
pub fn human_bytes(n: f64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n.max(0.0);
    let mut i = 0usize;
    while v >= 1024.0 && i + 1 < units.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", v as u64, units[i])
    } else {
        format!("{v:.1} {}", units[i])
    }
}

/// 一次采样的完整载荷（送前端 / 托盘 tooltip）。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Tick {
    /// 下行字节/秒（物理网卡合计）
    pub down: u64,
    /// 上行字节/秒
    pub up: u64,
    pub cpu: f64,
    /// 内存占用百分比
    pub mem: f64,
    /// 已用内存 GiB
    pub mem_used_gb: f64,
    pub disk_r: u64,
    pub disk_w: u64,
    pub today_sent: i64,
    pub today_recv: i64,
    /// WiFi 信号质量 0~100（未连接/无网卡为 None）。
    /// `#[serde(default)]`：老前端读新后端（或反之）时不会因缺字段报错。
    #[serde(default)]
    pub wifi_signal: Option<u32>,
    /// 当前 WiFi SSID（未连接为空串）
    #[serde(default)]
    pub wifi_ssid: String,
}

/// 遥测采样器：持有跨拍基线。
pub struct Sampler {
    tickno: u64,
    last_net: Option<BTreeMap<String, IfCounters>>,
    last_cpu: Option<(u64, u64, u64)>,
    last_disk: Option<(u64, u64)>,
    /// 磁盘计数器是否已成功取到（用于区分「无磁盘」与「首拍」）
    disk_ok: bool,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            tickno: 0,
            last_net: None,
            last_cpu: None,
            last_disk: None,
            disk_ok: false,
        }
    }

    pub fn tickno(&self) -> u64 {
        self.tickno
    }

    /// 采样一次。速率/CPU/磁盘均为「相对上一拍」的增量，首拍这些字段为 0。
    ///
    /// `today` 为今日 (sent, recv) 累计，由调用方从 TrafficStore 取（本模块不持有存储）。
    pub fn sample(&mut self, today: (i64, i64)) -> Tick {
        self.tickno += 1;
        let mut t = Tick {
            today_sent: today.0,
            today_recv: today.1,
            ..Default::default()
        };

        let cur_net = sample_interfaces();
        if let Some(last) = &self.last_net {
            let (down, up) = compute_rates(last, &cur_net);
            t.down = down;
            t.up = up;
        }
        self.last_net = Some(cur_net);

        let cur_cpu = sample_cpu_times();
        if let (Some(prev), Some(cur)) = (self.last_cpu, cur_cpu) {
            t.cpu = cpu_percent(prev, cur);
        }
        if cur_cpu.is_some() {
            self.last_cpu = cur_cpu;
        }

        if let Some(mem) = sample_memory() {
            t.mem = mem.0;
            t.mem_used_gb = mem.1;
        }

        let cur_disk = sample_disk_io();
        if let Some(cur) = cur_disk {
            if self.disk_ok {
                if let Some(prev) = self.last_disk {
                    t.disk_r = cur.0.saturating_sub(prev.0);
                    t.disk_w = cur.1.saturating_sub(prev.1);
                }
            }
            // 首拍（或计数器刚可用）报 0，不把开机以来的累计量当瞬时值
            self.last_disk = Some(cur);
            self.disk_ok = true;
        } else {
            self.disk_ok = false;
            self.last_disk = None;
        }
        t
    }
}

// ---------------- Win32 取数 ----------------

/// 取全部网卡计数器（别名 → 字节数）。
pub fn sample_interfaces() -> BTreeMap<String, IfCounters> {
    use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
    let mut out = BTreeMap::new();
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).0 != 0 || table.is_null() {
            return out;
        }
        let entries = (*table).NumEntries as usize;
        // Table 是 [MIB_IF_ROW2; 1] 的柔性数组：按首元素地址做指针偏移读取
        let base = std::ptr::addr_of!((*table).Table)
            as *const windows::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2;
        for i in 0..entries {
            let row = &*base.add(i);
            let alias = utf16_to_string(&row.Alias);
            if alias.is_empty() {
                continue;
            }
            out.insert(
                alias,
                IfCounters {
                    recv: row.InOctets,
                    sent: row.OutOctets,
                },
            );
        }
        FreeMibTable(table as *const core::ffi::c_void);
    }
    out
}

/// 名称是否属于「物理且已连接」的网卡（M3 的「无活动网络连接」判定用）。
pub fn physical_up_adapters() -> Vec<String> {
    use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    let mut out = Vec::new();
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).0 != 0 || table.is_null() {
            return out;
        }
        let entries = (*table).NumEntries as usize;
        let base = std::ptr::addr_of!((*table).Table)
            as *const windows::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2;
        for i in 0..entries {
            let row = &*base.add(i);
            let alias = utf16_to_string(&row.Alias);
            if alias.is_empty() || !is_physical(&alias) {
                continue;
            }
            if row.OperStatus == IfOperStatusUp {
                out.push(alias);
            }
        }
        FreeMibTable(table as *const core::ffi::c_void);
    }
    out
}

/// 取 CPU 累计 (idle, kernel, user) 的 100ns 计数。
pub fn sample_cpu_times() -> Option<(u64, u64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    unsafe {
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()?;
        Some((filetime_u64(idle), filetime_u64(kernel), filetime_u64(user)))
    }
}

fn filetime_u64(ft: windows::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

/// 取 (内存占用百分比, 已用 GiB)。
pub fn sample_memory() -> Option<(f64, f64)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    unsafe {
        let mut st = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        GlobalMemoryStatusEx(&mut st).ok()?;
        let total = st.ullTotalPhys as f64;
        let avail = st.ullAvailPhys as f64;
        let used = (total - avail).max(0.0);
        let pct = if total > 0.0 {
            used * 100.0 / total
        } else {
            0.0
        };
        Some((pct, used / (1024.0 * 1024.0 * 1024.0)))
    }
}

/// 取磁盘累计 (读字节, 写字节)。用 `IOCTL_DISK_PERFORMANCE` 逐物理盘累加。
///
/// 失败返回 None（调用方按「本拍无磁盘数据」处理，不清零既有基线）。
pub fn sample_disk_io() -> Option<(u64, u64)> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    // DISK_PERFORMANCE 里我们只关心 BytesRead / BytesWritten 两个 u64；
    // 完整结构体较大，这里按官方布局取前 16 字节（结构体开头即为这两个字段）。
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct DiskPerformanceHead {
        bytes_read: i64,
        bytes_written: i64,
    }
    const IOCTL_DISK_PERFORMANCE: u32 = 0x0007_0008;

    let mut total_read = 0u64;
    let mut total_write = 0u64;
    let mut any = false;
    for idx in 0..16 {
        let path = HSTRING::from(format!(r"\\.\PhysicalDrive{idx}"));
        unsafe {
            let h = match CreateFileW(
                &path,
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                Default::default(),
                None,
            ) {
                Ok(h) if h != INVALID_HANDLE_VALUE => h,
                _ => continue,
            };
            let mut head = DiskPerformanceHead::default();
            let mut returned = 0u32;
            let ok = DeviceIoControl(
                h,
                IOCTL_DISK_PERFORMANCE,
                None,
                0,
                Some(&mut head as *mut _ as *mut core::ffi::c_void),
                std::mem::size_of::<DiskPerformanceHead>() as u32,
                Some(&mut returned),
                None,
            )
            .is_ok();
            let _ = CloseHandle(h);
            if ok && returned as usize >= std::mem::size_of::<DiskPerformanceHead>() {
                total_read += head.bytes_read.max(0) as u64;
                total_write += head.bytes_written.max(0) as u64;
                any = true;
            }
        }
    }
    any.then_some((total_read, total_write))
}

fn utf16_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, u64, u64)]) -> BTreeMap<String, IfCounters> {
        pairs
            .iter()
            .map(|(n, r, s)| (n.to_string(), IfCounters { recv: *r, sent: *s }))
            .collect()
    }

    #[test]
    fn exclusion_list_matches_python_exactly() {
        assert_eq!(NIC_EXCLUDE.len(), 11);
        for must in [
            "loopback",
            "vmnet",
            "vmware",
            "vethernet",
            "bluetooth",
            "wan miniport",
            "tap",
            "wintun",
            "utun",
            "vgate",
            "hyper-v",
        ] {
            assert!(NIC_EXCLUDE.contains(&must), "缺少过滤关键字 {must}");
        }
    }

    #[test]
    fn is_physical_filters_virtual_adapters() {
        assert!(is_physical("以太网"));
        assert!(is_physical("WLAN"));
        assert!(is_physical("Ethernet 2"));
        // 大小写不敏感
        assert!(!is_physical("Loopback"));
        assert!(!is_physical("VMware Network Adapter VMnet8"));
        assert!(!is_physical("vEthernet (Default Switch)"));
        assert!(!is_physical("Hyper-V Virtual Ethernet Adapter"));
        assert!(!is_physical("WAN Miniport (IP)"));
        assert!(!is_physical("wintun"));
        assert!(!is_physical("Bluetooth Network Connection"));
        // 已知边界（与 Python 行为一致，不是漏判）：
        // 过滤表只含 "tap"，而 "TUN" 不含 "tap" —— Python 对
        // "Clash TUN" 同样判为物理。这是既有口径，改动会让两版曲线分叉，
        // 因此这里**锁住现状**而不是「修正」它。
        assert!(is_physical("Clash TUN"), "与 Python 一致：TUN 不在过滤表内");
        assert!(!is_physical("OpenVPN TAP-Windows6"), "含 tap 的才被排除");
    }

    #[test]
    fn rates_sum_only_physical_with_baseline() {
        // 过滤表是**英文/驱动名关键字**（vmware/tap/wintun…）。中文别名「新增虚拟网卡」
        // 不含任何关键字，因此按 Python 口径**算物理网卡**——本测试正是锁住这个事实：
        // 过滤靠关键字而不是「名字里有没有虚拟二字」的语义猜测。
        let last = map(&[
            ("以太网", 1000, 200),
            ("VMware Network Adapter VMnet8", 9999, 9999), // 含 vmware → 过滤
            ("新增虚拟网卡", 500, 500),                    // 无关键字 → 计入
        ]);
        let cur = map(&[
            ("以太网", 1600, 500),                           // +600 下行 / +300 上行
            ("VMware Network Adapter VMnet8", 99999, 99999), // 被过滤，贡献 0
            ("新增虚拟网卡", 900, 900),                      // 计入 → +400/+400
        ]);
        assert_eq!(compute_rates(&last, &cur), (1000, 700));
        // 断言「被过滤的那条贡献恰好为 0」：把它单独拿出来对比
        let without_vmnet = compute_rates(
            &map(&[("以太网", 1000, 200), ("新增虚拟网卡", 500, 500)]),
            &map(&[("以太网", 1600, 500), ("新增虚拟网卡", 900, 900)]),
        );
        assert_eq!(
            compute_rates(&last, &cur),
            without_vmnet,
            "含 vmware 的网卡必须完全不计入"
        );
    }

    #[test]
    fn rates_skip_adapters_missing_from_baseline() {
        // 与 Python `_fmt_nic_rates` 的 `p = last.get(name); if not p: continue` 一致：
        // 当前快照里有、上一拍没有的网卡**整条跳过**。否则刚插上的网卡会把开机以来的
        // 累计字节当成一秒内的增量，曲线瞬间飙到几十 GB。
        let last = map(&[("以太网", 1000, 1000)]);
        let cur = map(&[
            ("以太网", 1100, 1100),
            ("刚插上的网卡", 9_000_000, 9_000_000),
        ]);
        assert_eq!(
            compute_rates(&last, &cur),
            (100, 100),
            "无基线网卡必须跳过，只算以太网"
        );
    }

    #[test]
    fn rates_clamp_negative_delta_to_zero_per_direction() {
        // 网卡重置/计数器回绕：绝不能出现「负速率」或借位成巨大正数。
        // 夹零是**按方向独立**的：下行倒退归 0，同一拍的上行正增量照常计入。
        let last = map(&[("以太网", 1000, 1000)]);
        let cur = map(&[("以太网", 10, 1100)]);
        assert_eq!(compute_rates(&last, &cur), (0, 100));
    }

    #[test]
    fn rates_with_empty_inputs() {
        assert_eq!(compute_rates(&BTreeMap::new(), &BTreeMap::new()), (0, 0));
        assert_eq!(
            compute_rates(&map(&[("以太网", 0, 0)]), &BTreeMap::new()),
            (0, 0),
            "当前快照为空 → 无速率"
        );
        assert_eq!(
            compute_rates(&BTreeMap::new(), &map(&[("以太网", 5, 5)])),
            (0, 0),
            "无基线 → 首拍不报速率（避免把开机累计当瞬时暴增）"
        );
    }
    #[test]
    fn cpu_percent_basic_math() {
        // 上一拍 (idle=100, kernel=200, user=100)；本拍 (idle=150, kernel=250, user=150)
        // 增量：idle=50, kernel=50, user=50 → total=100, busy=50 → 50%
        assert_eq!(cpu_percent((100, 200, 100), (150, 250, 150)), 50.0);
    }

    #[test]
    fn cpu_percent_fully_idle_and_fully_busy() {
        // 全 idle：kernel 增量全在 idle 里
        assert_eq!(cpu_percent((0, 0, 0), (100, 100, 0)), 0.0);
        // 全忙：idle 无增量
        assert_eq!(cpu_percent((0, 0, 0), (0, 100, 100)), 100.0);
    }

    #[test]
    fn cpu_percent_never_divides_by_zero_nor_goes_negative() {
        assert_eq!(
            cpu_percent((10, 20, 30), (10, 20, 30)),
            0.0,
            "同 tick 重复采样"
        );
        assert_eq!(
            cpu_percent((100, 200, 300), (10, 20, 30)),
            0.0,
            "计数器倒退不产生负值"
        );
    }

    #[test]
    fn human_bytes_matches_python_buckets() {
        assert_eq!(human_bytes(0.0), "0 B");
        assert_eq!(human_bytes(512.0), "512 B");
        assert_eq!(human_bytes(1024.0), "1.0 KB");
        assert_eq!(human_bytes(1536.0), "1.5 KB");
        assert_eq!(human_bytes(1024.0 * 1024.0), "1.0 MB");
        assert_eq!(human_bytes(1024.0 * 1024.0 * 1024.0), "1.0 GB");
        assert_eq!(human_bytes(-5.0), "0 B", "负数夹零");
    }

    #[test]
    fn sampler_first_tick_has_no_rates_then_reports_deltas() {
        let mut s = Sampler::new();
        assert_eq!(s.tickno(), 0);
        let t1 = s.sample((10, 20));
        assert_eq!(s.tickno(), 1);
        // 首拍：速率必须为 0（无基线），但今日累计要带上
        assert_eq!((t1.down, t1.up), (0, 0), "首拍不得报速率");
        assert_eq!((t1.today_sent, t1.today_recv), (10, 20));
        assert!(
            t1.cpu >= 0.0 && t1.cpu <= 100.0,
            "cpu 必须在合法区间: {}",
            t1.cpu
        );
        assert!(
            t1.mem >= 0.0 && t1.mem <= 100.0,
            "mem 必须在合法区间: {}",
            t1.mem
        );
        assert!(t1.mem_used_gb >= 0.0);
        let t2 = s.sample((11, 22));
        assert_eq!(s.tickno(), 2);
        assert!(t2.cpu >= 0.0, "第二拍起 CPU 才有意义");
    }

    #[test]
    fn live_samples_are_sane_on_this_machine() {
        // 真机取数：只断言区间与不 panic，不断言具体数值（那是真机清单的事）
        let nics = sample_interfaces();
        assert!(!nics.is_empty(), "本机应至少有一个网卡");
        assert!(
            nics.keys().all(|n| !n.is_empty()),
            "网卡别名不得为空（为空说明 Alias 解码失败）"
        );
        let cpu = sample_cpu_times();
        assert!(cpu.is_some(), "GetSystemTimes 应可用");
        let mem = sample_memory().expect("GlobalMemoryStatusEx 应可用");
        assert!((0.0..=100.0).contains(&mem.0), "内存百分比: {}", mem.0);
        assert!(mem.1 > 0.0, "已用内存应为正");
        // 磁盘：允许为空（无权限/无物理盘），但不得 panic
        let _ = sample_disk_io();
        let ups = physical_up_adapters();
        // 不联网的机器也可能为空，只断言不会 panic 且名字都是物理网卡
        assert!(ups.iter().all(|n| is_physical(n)));
    }
}
