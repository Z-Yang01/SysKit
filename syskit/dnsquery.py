"""自研 UDP DNS 客户端：可指定 DNS 服务器做对比测试（无需第三方库）。"""
import os
import socket
import struct
import threading
from concurrent.futures import ThreadPoolExecutor


def _encode_qname(host: str) -> bytes:
    """按 DNS 线格式编码域名。

    长度字节必须等于「编码后的字节数」：中文等非 ASCII 标签经 IDNA 变成
    xn-- 前缀的 punycode，字节数与字符数不同——用 len(label) 会写出非法报文。
    """
    out = bytearray()
    for label in host.rstrip(".").split("."):
        enc = label.encode("ascii") if label.isascii() else label.encode("idna")
        if not enc or len(enc) > 63:
            raise ValueError(f"非法 DNS 标签: {label!r}")
        out.append(len(enc))
        out += enc
    out.append(0)
    return bytes(out)


def query(host: str, server: str, timeout: float = 3.0, qtype: int = 1) -> list:
    """向指定 DNS 服务器查询 A 记录（qtype=1）或 AAAA（28），返回 IP 列表；失败抛异常。"""
    txid = int.from_bytes(os.urandom(2), "big")
    qname = _encode_qname(host)
    packet = struct.pack(">HHHHHH", txid, 0x0100, 1, 0, 0, 0) + qname + struct.pack(">HH", qtype, 1)

    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.settimeout(timeout)
    try:
        s.sendto(packet, (server, 53))
        data, _ = s.recvfrom(4096)
    finally:
        s.close()

    rid, flags, qd, an, _, _ = struct.unpack(">HHHHHH", data[:12])
    if rid != txid or an == 0:
        raise RuntimeError("DNS 响应异常 (id 不匹配或无应答记录)")
    rcode = flags & 0x0F
    if rcode != 0:
        raise RuntimeError(f"DNS 返回错误码 {rcode}")

    # 跳过 Question 区
    pos = 12
    for _ in range(qd):
        while data[pos] != 0:
            if data[pos] & 0xC0:
                pos += 2
                break
            pos += data[pos] + 1
        else:
            pos += 1
        pos += 4

    # 解析 Answer 区
    ips = []
    for _ in range(an):
        while True:
            length = data[pos]
            if (length & 0xC0) == 0xC0:
                pos += 2
                break
            pos += 1
            if length == 0:
                break
            pos += length
        rtype, _class, _ttl, rdlen = struct.unpack(">HHIH", data[pos:pos + 10])
        pos += 10
        rd = data[pos:pos + rdlen]
        pos += rdlen
        if rtype == 1 and rdlen == 4:
            ips.append(socket.inet_ntoa(rd))
        elif rtype == 28 and rdlen == 16:
            ips.append(socket.inet_ntop(socket.AF_INET6, rd))
    if not ips:
        raise RuntimeError("应答中没有 A/AAAA 记录")
    return ips


def query_safe(host: str, server: str, timeout: float = 3.0) -> list:
    try:
        return query(host, server, timeout)
    except Exception:
        return []


def resolve_system(host: str, timeout: float = 4.0) -> list:
    """走系统解析器（getaddrinfo 无超时参数，放线程里限时等待）。"""
    result = {}

    def _work():
        try:
            infos = socket.getaddrinfo(host, None, socket.AF_INET)
            result["ips"] = sorted({i[4][0] for i in infos})
        except Exception as e:
            result["err"] = str(e)

    # 不用 with：ThreadPoolExecutor.__exit__ 会 shutdown(wait=True)，
    # 超时后仍阻塞到 getaddrinfo 结束，抵消超时语义。
    ex = ThreadPoolExecutor(max_workers=1)
    try:
        fut = ex.submit(_work)
        try:
            fut.result(timeout=timeout)
        except Exception:
            return []
    finally:
        ex.shutdown(wait=False)
    return result.get("ips", [])
