"""WLAN 信道推荐（纯逻辑，无 Qt）：干扰加权评分，供 WiFi 工具页调用。

2.4GHz：候选 1/6/11（经典非重叠三信道）。邻频重叠是干扰大头——按 802.11
频谱掩码近似，间隔 d=0..4 的能量权重约 1.0/0.53/0.22/0.08/0.02（d≥5 无
明显重叠）；每个 AP 的贡献 = 频谱权重 × 信号强度（netsh Signal %）。
旧实现只数同频 AP 且不分强弱：隔壁 -40dBm 的信道 3 AP 与远端 -90dBm 的
同频 AP 影响相同，推荐常被引到强干扰源旁边。

5GHz：20MHz 信道两两不重叠，但现代路由默认 80MHz 频宽——按 80MHz 块
评分（块内任一信道的 AP 都算占用），候选取各块主信道锚点
36/52/100/116/132/149。52~144 为 DFS 雷达信道（部分客户端不扫、须雷达
检测静默期），加罚 1.0（约合一个满强 AP）：非 DFS 明显不空时才推荐 DFS。
旧实现只在「扫到 AP 的信道」里挑最少——完全空闲的信道（最优解）永远
轮不到。

信号缺失/越界按 0 计；channel 缺失（含 0/None）的条目跳过；band 缺省时
按信道号推导（与 wifi_page 解析口径一致：1~14 为 2.4G，>14 为 5G）。
"""
_24G_CANDIDATES = (1, 6, 11)

# |Δch| = 0..4 的重叠能量权重（802.11 频谱掩码近似值）
_SPECTRUM_MASK = (1.0, 0.53, 0.22, 0.08, 0.02)

# 5G 80MHz 块 → 主信道锚点（165 非标准 80M 块，不推荐）
_5G_BLOCKS = ((36, 40, 44, 48), (52, 56, 60, 64), (100, 104, 108, 112),
              (116, 120, 124, 128), (132, 136, 140, 144), (149, 153, 157, 161))
_DFS_CHANNELS = frozenset(range(52, 145))
_DFS_PENALTY = 1.0


def _band(ap) -> str:
    band = ap.get("band")
    if band:
        return band
    ch = int(ap.get("channel") or 0)
    return "2.4G" if 0 < ch <= 14 else ("5G" if ch > 14 else "?")


def _weight(ap) -> float:
    """信号强度因子：netsh Signal %（0~100）→ 0~1，越界钳位，缺失按 0。"""
    try:
        return max(0, min(100, int(ap.get("signal") or 0))) / 100.0
    except (TypeError, ValueError):
        return 0.0


def _channel(ap) -> int:
    try:
        return int(ap.get("channel") or 0)
    except (TypeError, ValueError):
        return 0


def score_24g(aps, channel: int) -> float:
    """候选信道的干扰分：Σ 频谱权重(Δch) × 信号因子。越低越好。"""
    total = 0.0
    for ap in aps:
        d = abs(_channel(ap) - channel)
        if 0 <= d < len(_SPECTRUM_MASK):
            total += _SPECTRUM_MASK[d] * _weight(ap)
    return total


def recommend_channels(aps) -> dict:
    """扫描结果 → 推荐信息。返回：
    {"24g": 信道, "24g_same": 同频 AP 数, "24g_adj": ±4 内邻频 AP 数,
     "5g": 锚点信道或 None（无 5G AP）, "5g_block": 该 80M 块内 AP 数}。
    """
    a24 = [ap for ap in aps if _band(ap) == "2.4G" and _channel(ap)]
    a5 = [ap for ap in aps if _band(ap) == "5G" and _channel(ap)]

    # 平分时取先到者（1<6<11）：全空环境惯例从 1 起荐
    best24 = min(_24G_CANDIDATES, key=lambda c: score_24g(a24, c))
    same24 = sum(1 for ap in a24 if _channel(ap) == best24)
    adj24 = sum(1 for ap in a24 if 0 < abs(_channel(ap) - best24) <= 4)

    best5 = None
    block5 = 0
    if a5:
        best_score = None
        for block in _5G_BLOCKS:
            anchor = block[0]
            s = sum(_weight(ap) for ap in a5 if _channel(ap) in block)
            if anchor in _DFS_CHANNELS:
                s += _DFS_PENALTY
            if best_score is None or s < best_score:   # 平分取低信道、非 DFS 优先（罚分保证）
                best_score, best5 = s, anchor
                block5 = sum(1 for ap in a5 if _channel(ap) in block)
    return {"24g": best24, "24g_same": same24, "24g_adj": adj24,
            "5g": best5, "5g_block": block5}
