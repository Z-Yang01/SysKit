"""winreg 封装：统一 'HKLM\路径' 格式读写。"""
import winreg

_HIVES = {"HKLM": winreg.HKEY_LOCAL_MACHINE, "HKCU": winreg.HKEY_CURRENT_USER,
          "HKCR": winreg.HKEY_CLASSES_ROOT, "HKU": winreg.HKEY_USERS}


def _split(path: str):
    hive, _, sub = path.partition("\\")
    return _HIVES[hive], sub


def values(path: str) -> dict:
    """读某键下全部值 → {名称: 数据}。DWORD 自动转 int。

    任何读取失败（键不存在/权限不足/未知根键）都返回空 dict，绝不向上抛：
    本函数只做只读探测，供诊断与状态展示，不应因注册表异常打断调用方。
    """
    try:
        hive, sub = _split(path)
        k = winreg.OpenKey(hive, sub, 0, winreg.KEY_READ)
    except Exception:
        return {}
    out = {}
    try:
        i = 0
        while True:
            try:
                name, data, _t = winreg.EnumValue(k, i)
            except OSError:
                break
            out[name] = int(data) if isinstance(data, int) else data
            i += 1
    except Exception:
        pass
    finally:
        try:
            k.Close()
        except Exception:
            pass
    return out


def value(path: str, name: str, default=None):
    vals = values(path)
    return vals.get(name, default) if vals else default


def subkeys(path: str) -> list:
    try:
        hive, sub = _split(path)
        k = winreg.OpenKey(hive, sub, 0, winreg.KEY_READ)
    except Exception:
        return []
    out = []
    try:
        i = 0
        while True:
            try:
                out.append(winreg.EnumKey(k, i))
            except OSError:
                break
            i += 1
    except Exception:
        pass
    finally:
        try:
            k.Close()
        except Exception:
            pass
    return out


def pnp_states():
    """一次扫描网卡类键 → {DriverDesc: 'disabled' | 'enabled_off'}。

    类键下无任何子键（读取失败/权限不足）→ None，调用方据此判「注册表不可读」；
    读到子键但没有匹配的驱动描述 → {}（两者语义不同，不能合并成假值）。
    同描述对应多个驱动实例键时，任一键已禁止即视为已禁止。
    """
    base = r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}"
    subs = subkeys(base)
    if not subs:
        return None
    out = {}
    for sub in subs:
        if not re_key(sub):
            continue
        vals = values(f"{base}\\{sub}")
        desc = str(vals.get("DriverDesc", ""))
        if not desc:
            continue
        state = "disabled" if str(vals.get("PnPCapabilities")) == "24" else "enabled_off"
        if out.get(desc) != "disabled":
            out[desc] = state
    return out


def pnpcapabilities(driver_desc: str, scan: dict | None = None):
    """按 DriverDesc 取节电态：'enabled_off'(允许断电/默认) | 'disabled'(已禁止) | None(未找到)。
    （WMI 电源接口损坏时以此为准。）

    scan 可传入 pnp_states() 的批量结果：逐个网卡各自全量扫描是 O(网卡数 × 类键数)
    的重复枚举（实测 7ms/次 × 十几个网卡 ≈ 100ms），一次扫描查表即可。
    """
    if scan is None:
        scan = pnp_states() or {}
    return scan.get(driver_desc)


def re_key(s: str):
    return s and s.startswith("0")
