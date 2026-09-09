"""自研 UDP DNS 客户端：可指定 DNS 服务器做对比测试（无需第三方库）。"""
import os
import socket
import struct
import threading
from concurrent.futures import ThreadPoolExecutor


def query(host: str, server: str, timeout: float = 3.0, qtype: int = 1) -> list:
    """向指定 DNS 服务器查询 A 记录（qtype=1）或 AAAA（28），返回 IP 列表；失败抛异常。"""
    txid = int.from_bytes(os.urandom(2), "big")
    qname = b"".join(
        bytes([len(label)]) + label.encode("idna") if not label.isascii() else bytes([len(label)]) + label.encode()
        for label in host.split(".")
    ) + b"\x00"
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
            if length & 0xC0 == 0xC0:
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

    with ThreadPoolExecutor(max_workers=1) as ex:
        fut = ex.submit(_work)
        try:
            fut.result(timeout=timeout)
        except Exception:
            return []
    return result.get("ips", [])
