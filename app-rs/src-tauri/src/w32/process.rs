//! 子进程执行 / 注册表读写 / 提权判定 / 单实例互斥。
use super::*;

// ---------------- 子进程执行（修复动作的底座） ----------------

/// 子进程执行结果（对应 Python `proc.run` 的返回）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcOutput {
    pub ok: bool,
    pub code: u32,
    pub stdout: String,
    pub stderr: String,
}

impl ProcOutput {
    /// `stdout + stderr` 合并（Python 侧多处这样拼接做关键字判定）。
    pub fn combined(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    /// 尾部截断（Python 有 `r.stdout[-200:] + r.stderr[-100:]` 这类写法）。
    pub fn tail(&self, out_n: usize, err_n: usize) -> String {
        format!(
            "{}{}",
            tail_chars(&self.stdout, out_n),
            tail_chars(&self.stderr, err_n)
        )
    }
}

fn tail_chars(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        return s.to_string();
    }
    chars[chars.len() - n..].iter().collect()
}

/// 按 Windows 命令行规则给单个参数加引号（含空格/制表/引号时必须）。
///
/// 规则同 Python `subprocess.list2cmdline`：反斜杠只在引号前成对转义，
/// 参数末尾的反斜杠在加引号时要翻倍。
#[cfg(test)]
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
                out.push(ch);
            }
            '"' => {
                out.push_str(&"\\".repeat(backslashes));
                out.push_str("\\\"");
                backslashes = 0;
            }
            _ => {
                backslashes = 0;
                out.push(ch);
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes));
    out.push('"');
    out
}

/// 把参数表拼成命令行。
#[cfg(test)]
pub fn build_command_line(args: &[String]) -> String {
    args.iter()
        .map(|a| quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 把命令名解析成绝对路径。
///
/// **为什么必须自己做**：一旦给 `CreateProcessW` 传了 `lpApplicationName`，
/// Windows 就**不再搜索 PATH**（只搜索当前目录）——`cmd`、`ipconfig`、`netsh`
/// 这些裸名字全都会报「系统找不到指定的文件」。Python 的 subprocess 会自己解析
/// PATH，所以这是移植时必须补上的一层。
///
/// 解析顺序：含路径分隔符 → 原样返回；否则按 PATHEXT 在 PATH 各目录里找。
pub fn resolve_executable(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if name.contains('\\') || name.contains('/') {
        let p = std::path::Path::new(name);
        return p.is_file().then(|| name.to_string());
    }
    // PATHEXT 默认值（Windows 未设 PATHEXT 时也要能用）
    let exts: Vec<String> = std::env::var("PATHEXT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.split(';')
                .map(|e| e.trim().to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect()
        })
        .unwrap_or_else(|| vec![".com".into(), ".exe".into(), ".bat".into(), ".cmd".into()]);
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        // 已经带扩展名的先直接试
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct.to_string_lossy().to_string());
        }
        for ext in &exts {
            let cand = dir.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Some(cand.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// 读取注册表值（字符串 / DWORD / 多字符串 / 二进制）；键或值不存在返回 None。
///
/// 对应 Python `winregread.value(path, name, default)`。路径形如
/// `HKLM\SYSTEM\CurrentControlSet\Services\NlaSvc\Parameters\Internet`。
#[allow(dead_code)] // 注册表读写 API 对齐面（winregread 语义）
pub fn reg_value(path: &str, name: &str) -> Option<RegValue> {
    let hkey = reg_open(path)?;
    let out = reg_query(hkey, name);
    unsafe {
        let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
    }
    out
}

/// 枚举一个键下的全部值（对应 Python `winregread.values(path)`）；失败返回空表。
/// 写 DWORD 值（对应 Python `winreg.SetValueEx(..., REG_DWORD, v)`）。
///
/// 需要 `KEY_SET_VALUE`：HKCU 通常无需提升，HKLM 需管理员——调用方负责判定，
/// 本层只返回成败，绝不静默吞掉失败。
pub fn reg_set_dword(path: &str, name: &str, value: u32) -> bool {
    let Some(hkey) = reg_open_write(path) else {
        return false;
    };
    let data = value.to_le_bytes();
    let rc = unsafe {
        windows::Win32::System::Registry::RegSetValueExW(
            hkey,
            &HSTRING::from(name),
            None,
            windows::Win32::System::Registry::REG_DWORD,
            Some(&data),
        )
    };
    unsafe {
        let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
    }
    rc == windows::Win32::Foundation::WIN32_ERROR(0)
}

/// 写字符串值（REG_SZ）。
pub fn reg_set_string(path: &str, name: &str, value: &str) -> bool {
    let Some(hkey) = reg_open_write(path) else {
        return false;
    };
    let mut wide: Vec<u16> = value.encode_utf16().collect();
    wide.push(0);
    let data: Vec<u8> = wide.iter().flat_map(|u| u.to_le_bytes()).collect();
    let rc = unsafe {
        windows::Win32::System::Registry::RegSetValueExW(
            hkey,
            &HSTRING::from(name),
            None,
            windows::Win32::System::Registry::REG_SZ,
            Some(&data),
        )
    };
    unsafe {
        let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
    }
    rc == windows::Win32::Foundation::WIN32_ERROR(0)
}

/// 写「以值名为规格」的注册表项（持久路由用：值名就是 `目标,掩码,网关,跃点`）。
///
/// 与 `reg_set_string` 的区别在于**先删同名旧值再写**：改路由时规格串本身是键名，
/// 不先删会留下旧路由（两条并存 → 依然劫持流量）。
#[allow(dead_code)] // 注册表读写 API 对齐面（winregread 语义）
pub fn reg_write_named_value(path: &str, name: &str, data: &str) -> bool {
    let _ = reg_delete_value(path, name);
    reg_set_string(path, name, data)
}

/// 通知 WinINET 代理设置已变更（对应 Python `wininet.InternetSetOptionW`）。
///
/// 39 = INTERNET_OPTION_SETTINGS_CHANGED，37 = INTERNET_OPTION_REFRESH。
/// 不刷新的话，已运行的应用仍用旧代理配置——用户会以为"改了没生效"。
pub fn notify_internet_settings_changed() -> bool {
    use windows::Win32::Networking::WinInet::InternetSetOptionW;
    unsafe {
        let a = InternetSetOptionW(None, 39, None, 0).is_ok();
        let b = InternetSetOptionW(None, 37, None, 0).is_ok();
        a && b
    }
}

/// 删除值。Ok(true) = 已删除，Ok(false) = 值本来就不存在，Err = 真失败。
///
/// **区分这三者很重要**：清 PAC 时「本来就没有」不是失败，而权限不足是失败。
pub fn reg_delete_value(path: &str, name: &str) -> Result<bool, String> {
    use windows::Win32::Foundation::WIN32_ERROR;
    const ERROR_FILE_NOT_FOUND: WIN32_ERROR = WIN32_ERROR(2);
    const ERROR_PATH_NOT_FOUND: WIN32_ERROR = WIN32_ERROR(3);
    let Some(hkey) = reg_open_write(path) else {
        return Err(format!("无法以写权限打开注册表键: {path}"));
    };
    let rc =
        unsafe { windows::Win32::System::Registry::RegDeleteValueW(hkey, &HSTRING::from(name)) };
    unsafe {
        let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
    }
    if rc == WIN32_ERROR(0) {
        Ok(true)
    } else if rc == ERROR_FILE_NOT_FOUND || rc == ERROR_PATH_NOT_FOUND {
        Ok(false)
    } else {
        Err(format!(
            "删除注册表值失败 {path}\\{name}（错误码 {}）",
            rc.0
        ))
    }
}

/// 以写权限打开键（读用 `reg_open`）。
fn reg_open_write(path: &str) -> Option<windows::Win32::System::Registry::HKEY> {
    use windows::Win32::System::Registry::{
        RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, KEY_WOW64_64KEY,
    };
    let (root, sub) = split_reg_path(path)?;
    let hkey = match root {
        "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
        "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
        _ => return None,
    };
    let mut out = HKEY(std::ptr::null_mut());
    let rc = unsafe {
        RegOpenKeyExW(
            hkey,
            &HSTRING::from(sub),
            Some(0),
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            &mut out,
        )
    };
    (rc == windows::Win32::Foundation::WIN32_ERROR(0)).then_some(out)
}

pub fn reg_values(path: &str) -> BTreeMap<String, RegValue> {
    use windows::Win32::System::Registry::{RegEnumValueW, RegQueryInfoKeyW};
    let mut out = BTreeMap::new();
    let Some(hkey) = reg_open(path) else {
        return out;
    };
    unsafe {
        let mut subkeys = 0u32;
        let mut values = 0u32;
        let mut max_name = 0u32;
        let mut max_data = 0u32;
        if RegQueryInfoKeyW(
            hkey,
            None,
            None,
            None,
            Some(&mut subkeys),
            None,
            None,
            Some(&mut values),
            Some(&mut max_name),
            Some(&mut max_data),
            None,
            None,
        ) != windows::Win32::Foundation::WIN32_ERROR(0)
        {
            let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
            return out;
        }
        let name_cap = (max_name + 2) as usize;
        let data_cap = (max_data + 8) as usize;
        for i in 0..values {
            let mut name_buf = vec![0u16; name_cap];
            let mut name_len = name_cap as u32;
            let mut ty = REG_VALUE_TYPE(0);
            let mut data = vec![0u8; data_cap];
            let mut data_len = data_cap as u32;
            let rc = RegEnumValueW(
                hkey,
                i,
                Some(windows::core::PWSTR(name_buf.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut ty as *mut REG_VALUE_TYPE as *mut u32),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            );
            if rc != windows::Win32::Foundation::WIN32_ERROR(0) {
                continue;
            }
            let key = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            if let Some(v) = decode_reg_value(ty, &data[..data_len as usize]) {
                out.insert(key, v);
            }
        }
        let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
    }
    out
}

/// 注册表值（保留类型，便于与默认值做字符串比对——Python 侧也是 `str(got) != str(want)`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegValue {
    Dword(u32),
    Str(String),
    MultiStr(Vec<String>),
    Binary(Vec<u8>),
}

impl RegValue {
    /// 与 Python `str(value)` 等价的展示形态（比对默认值时用）。
    pub fn as_compare_str(&self) -> String {
        match self {
            RegValue::Dword(n) => n.to_string(),
            RegValue::Str(s) => s.clone(),
            RegValue::MultiStr(v) => v.join(","),
            RegValue::Binary(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self {
            RegValue::Dword(n) => Some(*n),
            RegValue::Str(s) => s.trim().parse::<u32>().ok(),
            _ => None,
        }
    }
}

fn reg_open(path: &str) -> Option<windows::Win32::System::Registry::HKEY> {
    use windows::Win32::System::Registry::{
        RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY,
    };
    let (root, sub) = split_reg_path(path)?;
    let hkey = match root {
        "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
        "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
        _ => return None,
    };
    let mut out = HKEY(std::ptr::null_mut());
    // WOW64_64KEY：SysKit 是 64 位进程，显式读 64 位视图，避免被重定向到 Wow6432Node
    let rc = unsafe {
        RegOpenKeyExW(
            hkey,
            &HSTRING::from(sub),
            Some(0),
            KEY_READ | KEY_WOW64_64KEY,
            &mut out,
        )
    };
    (rc == windows::Win32::Foundation::WIN32_ERROR(0)).then_some(out)
}

fn split_reg_path(path: &str) -> Option<(&str, &str)> {
    let mut it = path.splitn(2, '\\');
    let root = it.next()?;
    let sub = it.next().unwrap_or("");
    (!root.is_empty()).then_some((root, sub))
}

#[allow(dead_code)] // 注册表读写 API 对齐面（winregread 语义）
fn reg_query(hkey: windows::Win32::System::Registry::HKEY, name: &str) -> Option<RegValue> {
    use windows::Win32::System::Registry::{RegQueryValueExW, REG_VALUE_TYPE};
    let mut ty = REG_VALUE_TYPE(0);
    let mut len = 0u32;
    let name_h = HSTRING::from(name);
    let rc = unsafe { RegQueryValueExW(hkey, &name_h, None, Some(&mut ty), None, Some(&mut len)) };
    if rc != windows::Win32::Foundation::WIN32_ERROR(0) || len == 0 {
        // 长度为 0 的合法值（空字符串）也要能取到：再走一次带缓冲的调用
        let mut buf = [0u8; 4];
        let mut l = buf.len() as u32;
        let rc2 = unsafe {
            RegQueryValueExW(
                hkey,
                &name_h,
                None,
                Some(&mut ty),
                Some(buf.as_mut_ptr()),
                Some(&mut l),
            )
        };
        if rc2 != windows::Win32::Foundation::WIN32_ERROR(0) {
            return None;
        }
        return decode_reg_value(ty, &buf[..l as usize]);
    }
    let mut buf = vec![0u8; len as usize + 2];
    let mut l = buf.len() as u32;
    let rc2 = unsafe {
        RegQueryValueExW(
            hkey,
            &name_h,
            None,
            Some(&mut ty),
            Some(buf.as_mut_ptr()),
            Some(&mut l),
        )
    };
    if rc2 != windows::Win32::Foundation::WIN32_ERROR(0) {
        return None;
    }
    decode_reg_value(ty, &buf[..l as usize])
}

fn decode_reg_value(
    ty: windows::Win32::System::Registry::REG_VALUE_TYPE,
    data: &[u8],
) -> Option<RegValue> {
    use windows::Win32::System::Registry::{REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_SZ};
    if ty == REG_DWORD {
        if data.len() < 4 {
            return None;
        }
        return Some(RegValue::Dword(u32::from_le_bytes([
            data[0], data[1], data[2], data[3],
        ])));
    }
    if ty == REG_SZ || ty == REG_EXPAND_SZ {
        return Some(RegValue::Str(utf16_bytes_to_string(data)));
    }
    if ty == REG_MULTI_SZ {
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let mut items = Vec::new();
        let mut cur = Vec::new();
        for u in units {
            if u == 0 {
                if cur.is_empty() {
                    break;
                }
                items.push(String::from_utf16_lossy(&cur));
                cur.clear();
            } else {
                cur.push(u);
            }
        }
        return Some(RegValue::MultiStr(items));
    }
    Some(RegValue::Binary(data.to_vec()))
}

fn utf16_bytes_to_string(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let end = units.iter().position(|c| *c == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// 当前进程是否以管理员（提升）身份运行。
///
/// 对应 Python `admin.is_admin()`：取令牌的 TokenElevation 判断。修复动作里
/// 「改配置类」在非管理员下必须跳过或明确失败，绝不能假装成功。
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE(std::ptr::null_mut());
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut core::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// 失败时返回人话原因（修复页要把它显示给用户，不能只报「失败了」）。
pub fn run_capture(args: &[String], timeout_secs: u64) -> Result<ProcOutput, String> {
    use std::process::{Command, Stdio};

    if args.is_empty() {
        return Err("空命令".to_string());
    }
    let resolved = resolve_executable(&args[0])
        .ok_or_else(|| format!("找不到可执行文件: {}（不在 PATH 中）", args[0]))?;

    let mut cmd = Command::new(&resolved);
    cmd.args(&args[1..]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // 不弹控制台黑窗（对应 Python 的 startupinfo 隐藏窗口）
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法启动 {}: {e}", args[0]))?;

    // 管道必须**边等边抽**（读线程）：子进程输出一旦超过管道缓冲就会阻塞在
    // write 上——「先等退出再读管道」的顺序会让它永远退不出，只能被超时 kill，
    // 修复动作被误报成失败。两个管道各挂一个读线程读到 EOF，等待循环只看
    // 进程状态；超时 kill 后管道关闭、线程自然收尾，无双死锁。
    // （std 的 wait_with_output 也能并发抽干，但它不带超时——这里需要超时 kill。）
    let mut stdout_pipe = child.stdout.take().expect("stdout 已声明 piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr 已声明 piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout_pipe, &mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stderr_pipe, &mut buf);
        buf
    });

    // 超时：std 没有 wait_timeout，轮询 try_wait 最省事且不引额外依赖。
    // 到点先 kill 再收尸，绝不留下孤儿进程（修复动作里有 90 秒级的 ipconfig /renew）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs.max(1));
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(format!("等待 {} 失败: {e}", args[0])),
        }
    }

    // 读线程在管道 EOF（正常退出或 kill 关句柄）后必然结束；join 只做收集
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    let status = child
        .wait()
        .map_err(|e| format!("读取 {} 状态失败: {e}", args[0]))?;

    let code = status.code().unwrap_or(-1);
    Ok(ProcOutput {
        ok: !timed_out && status.success(),
        code: if timed_out {
            u32::MAX
        } else {
            code.max(0) as u32
        },
        stdout: decode_console_bytes(&stdout),
        stderr: decode_console_bytes(&stderr),
    })
}

/// 控制台输出解码：先试 UTF-8，失败退化为逐字节（保 ASCII 可读，绝不 panic）。
///
/// 中文 Windows 控制台默认 GBK，完整码表需额外依赖；这里保证「修复杂志不出现
/// 替换字符乱码致无法判断成败」这一底线，同时 ASCII 部分（含错误码）始终可读。
fn decode_console_bytes(raw: &[u8]) -> String {
    if raw.is_empty() {
        return String::new();
    }
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => raw.iter().map(|b| *b as char).collect::<String>(),
    }
}

/// 单实例互斥：创建命名互斥量并立即持有关闭句柄的能力由调用方决定——
/// 这里只创建；若 ERROR_ALREADY_EXISTS 则返回 false（已有实例）。
/// 句柄有意泄漏（与进程同寿），进程退出时系统自动回收。
pub fn acquire_single_instance_mutex(name: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    unsafe {
        let w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let h = CreateMutexW(None, false, PCWSTR(w.as_ptr()));
        if h.is_err() {
            return false;
        }
        // 不 CloseHandle：互斥量必须与进程同寿（释放即失去单实例保护）
        std::mem::forget(h);
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}
