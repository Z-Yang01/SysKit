"""ICMP ping：ctypes 调 iphlpapi IcmpSendEcho，返回毫秒或 None（丢包/失败）。

相比解析 ping.exe 输出：无本地化问题、无子进程开销、精度更高。
注意 ReplyBuffer 必须容纳 ICMP 差错响应（IP头≤60 + 8 字节），否则缓冲区溢出。
"""
import ctypes
import socket
import struct

_iphlpapi = ctypes.windll.iphlpapi


class _ICMP_ECHO_REPLY(ctypes.Structure):
    _fields_ = [("Address", ctypes.c_uint32),
                ("Status", ctypes.c_uint32),
                ("RoundTripTime", ctypes.c_uint32),
                ("DataSize", ctypes.c_uint16),
                ("Reserved", ctypes.c_uint16),
                ("Data", ctypes.c_void_p),
                ("Options", ctypes.c_ubyte * 16)]


_iphlpapi.IcmpCreateFile.restype = ctypes.c_void_p
_iphlpapi.IcmpSendEcho.argtypes = [
    ctypes.c_void_p, ctypes.c_uint32, ctypes.c_char_p, ctypes.c_uint16,
    ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32]
_iphlpapi.IcmpSendEcho.restype = ctypes.c_uint32
_iphlpapi.IcmpCloseHandle.argtypes = [ctypes.c_void_p]

_PAYLOAD = b"NetToolPingData"
_INVALID = 0xFFFFFFFFFFFFFFFF


def ping(host: str, timeout_ms: int = 1500):
    """ping 指定主机，成功返回延迟毫秒数（float），失败/超时返回 None。"""
    try:
        ip = socket.gethostbyname(host)
    except OSError:
        return None
    handle = _iphlpapi.IcmpCreateFile()
    if not handle or handle == _INVALID:
        return None
    try:
        # 结构体 + 回显负载 + 差错响应预留（IP头60+8）
        reply_size = ctypes.sizeof(_ICMP_ECHO_REPLY) + len(_PAYLOAD) + 68 + 16
        buf = ctypes.create_string_buffer(reply_size)
        dest = struct.unpack("<I", socket.inet_aton(ip))[0]
        ret = _iphlpapi.IcmpSendEcho(
            handle, dest,
            _PAYLOAD, len(_PAYLOAD),
            None,
            buf, reply_size,
            timeout_ms,
        )
        if ret == 0:
            return None
        reply = ctypes.cast(buf, ctypes.POINTER(_ICMP_ECHO_REPLY)).contents
        if reply.Status != 0:  # 0 = IP_SUCCESS
            return None
        return float(reply.RoundTripTime)
    except Exception:
        return None
    finally:
        _iphlpapi.IcmpCloseHandle(handle)


def default_gateway() -> str:
    """从路由表取当前默认网关（经 PowerShell，启动/网络变化时调用）。"""
    from . import proc
    r = proc.run_ps("Get-NetRoute -DestinationPrefix 0.0.0.0/0 -ErrorAction SilentlyContinue "
                    "| Sort-Object {$_.RouteMetric+$_.InterfaceMetric} "
                    "| Select-Object -First 1 -ExpandProperty NextHop", timeout=25)
    gw = r.stdout.strip()
    return gw if gw.count(".") == 3 else ""
