"""WLAN 信道推荐（syskit/wifi.py）：干扰加权评分的鉴别性用例。

每个用例注释里给出旧算法（只数同频 AP / 5G 只在扫到的信道里挑）的结论，
标明新算法为何更优。纯逻辑零 Qt，不依赖扫描器。
"""
from syskit.wifi import recommend_channels, score_24g


def _ap(ch, sig, band=None):
    ap = {"channel": ch, "signal": sig}
    if band:
        ap["band"] = band
    return ap


# ---------------- 频谱掩码计权 ----------------

class TestScore24g:
    def test_co_channel_full_signal(self):
        assert score_24g([_ap(6, 100)], 6) == 1.0

    def test_signal_scales_linearly(self):
        assert score_24g([_ap(6, 50)], 6) == 0.5

    def test_bad_entries_zero_weight(self):
        assert score_24g([{"channel": None, "signal": 100},
                          {"channel": "x", "signal": 99},
                          {"channel": 6, "signal": None},
                          {"channel": 6, "signal": "abc"}], 6) == 0.0
        assert score_24g([{"channel": 200, "signal": 100}], 6) == 0.0  # 远超 ±4 无重叠
        assert score_24g([{"channel": 6, "signal": 150}], 6) == 1.0    # 信号越界钳到 100

    def test_adjacent_mask_decay(self):
        # 间隔 d=1..4 → 0.53/0.22/0.08/0.02；d≥5 无重叠不计
        assert score_24g([_ap(5, 100)], 6) == 0.53
        assert score_24g([_ap(4, 100)], 6) == 0.22
        assert score_24g([_ap(3, 100)], 6) == 0.08
        assert score_24g([_ap(2, 100)], 6) == 0.02
        assert score_24g([_ap(1, 100)], 6) == 0.0
        assert score_24g([_ap(11, 100)], 6) == 0.0


# ---------------- 推荐结果 ----------------

class TestRecommend24g:
    def test_empty_environment_defaults_to_1(self):
        rec = recommend_channels([])
        assert rec["24g"] == 1 and rec["24g_same"] == 0 and rec["24g_adj"] == 0

    def test_strong_adjacent_beats_naive_count(self):
        """鉴别性用例：ch2@95 强邻频 + ch5@10 弱邻频。
        旧算法三家同频全 0 → 惯性荐 1（紧挨着最强干扰源）；
        新算法频谱计权：1 得 0.52、6 得 0.07、11 得 0 → 荐 11。"""
        aps = [_ap(2, 95), _ap(5, 10)]
        rec = recommend_channels(aps)
        assert rec["24g"] == 11
        assert rec["24g_same"] == 0 and rec["24g_adj"] == 0  # 2/5 距 11 均 >4

    def test_single_adjacent_interferer(self):
        """仅 ch3@100 一个 AP：旧算法荐 1（与 6/11 并列全 0 取 1）；
        新算法 1 得 0.22、6 得 0.08、11 得 0 → 荐 11。"""
        assert recommend_channels([_ap(3, 100)])["24g"] == 11

    def test_same_and_adjacent_counts(self):
        """同频/邻频计数口径：best=11 时 ch9 计邻频（Δ2），ch1/4/6 均超 ±4 不计。"""
        aps = [_ap(6, 50), _ap(4, 60), _ap(9, 70), _ap(1, 80)]
        rec = recommend_channels(aps)
        assert rec["24g"] == 11
        assert rec["24g_same"] == 0 and rec["24g_adj"] == 1

    def test_band_derived_when_missing(self):
        """band 缺省按信道号推导（与 wifi_page 解析口径一致）。
        ch7@50：score1=0（Δ6 无重叠）< score11=0.01（Δ4）< score6=0.5（同频）→ 荐 1。"""
        assert recommend_channels([_ap(7, 50)])["24g"] == 1


class TestRecommend5g:
    def test_empty_environment_no_5g_recommendation(self):
        """无任何 5G AP → 5g=None（UI 显示「未发现 5G 网络」，与旧版契约一致）；
        2.4G 侧仍给默认推荐 1。"""
        rec = recommend_channels([])
        assert rec["5g"] is None
        assert rec["24g"] == 1 and rec["24g_same"] == 0 and rec["24g_adj"] == 0

    def test_empty_block_preferred_over_scanned_channel(self):
        """核心修复：ch36 有 AP 时旧算法只能荐 36（只在扫到的信道里挑）；
        新算法按 80M 块评分 → 全空非 DFS 锚点 149。"""
        rec = recommend_channels([_ap(36, 55, band="5G")])
        assert rec["5g"] == 149 and rec["5g_block"] == 0

    def test_block_counts_any_channel_in_80m(self):
        """块内任一信道（ch40）都污染锚点 36：40 的 AP 计入 36 块。"""
        rec = recommend_channels([_ap(40, 100, band="5G"), _ap(149, 100, band="5G")])
        # 36 块 1.0、52 空=0+1.0 罚、100/116/132 同、149 块 1.0 → 平分 1.0/1.0
        # 取先到者 36？——149 块 1.0 与 36 块 1.0 相等，锚点升序遍历 36 先到 → 36
        assert rec["5g"] == 36 and rec["5g_block"] == 1

    def test_dfs_chosen_when_non_dfs_clearly_crowded(self):
        """36 与 149 块各 2 个满强 AP（各 2.0）> DFS 罚 1.0：全空的 52 块（0+1.0）
        胜出——非 DFS 明显拥挤时才动用雷达信道。"""
        aps = [_ap(36, 100, band="5G"), _ap(40, 100, band="5G"),
               _ap(149, 100, band="5G"), _ap(153, 100, band="5G")]
        rec = recommend_channels(aps)
        assert rec["5g"] == 52 and rec["5g_block"] == 0

    def test_dfs_penalty_keeps_non_dfs_on_near_tie(self):
        """149 块仅 1 个 30% 弱 AP（0.3）< DFS 罚 1.0 → 仍荐 149（雷达信道不值得）。"""
        rec = recommend_channels([_ap(36, 100, band="5G"), _ap(149, 30, band="5G")])
        assert rec["5g"] == 149 and rec["5g_block"] == 1

    def test_channel_zero_and_unknown_band_ignored(self):
        """channel 0（wifi_page 口径同款脏数据）不入桶；band 缺省 ch>14 归 5G。"""
        rec = recommend_channels([{"ssid": "x", "channel": 0, "signal": 90},
                                  {"ssid": "y", "channel": 40, "signal": 100}])
        assert rec["5g"] == 149 and rec["5g_block"] == 0   # 40 污染 36 块 → 149


class TestAuditRound28BranchLocks:
    """无人值守轮次28 审计补锁：f8d03d1 新逻辑里零覆盖的四个分支——
    信号越界钳位（下沿）/ DFS 边界信道 52 与 144 / 165 不属任何 80M 块 /
    band 字段在场时优先于信道号推导。"""

    def test_negative_or_missing_signal_clamped_to_zero(self):
        """信号下沿钳位：-20 必须按 0 计（去掉 max(0,·) 会得 -0.2 负分，
        干扰源反而「降低」候选信道得分）；缺 signal 键同理按 0。"""
        assert score_24g([{"channel": 6, "signal": -20}], 6) == 0.0
        assert score_24g([{"channel": 6}], 6) == 0.0
        assert score_24g([{"channel": 7, "signal": -100}], 7) == 0.0

    def test_dfs_boundaries_52_and_144_both_penalized(self):
        """DFS 块边界钉死：下沿块 52-64、上沿块 132-144 都必须吃到 +1.0 罚
        且块成员 52/144 都按占用计权（罚按块锚点查 _DFS_CHANNELS，占用按
        块成员表）。十个满强 AP 把六个 80M 块全部顶到 2.0 → 六块并列，取
        先到者 36。若 52/144 所在块的锚点漏出 DFS 集、或成员被划出块表，
        对应块跌到 1.0 胜出 → 本用例红。"""
        aps = [_ap(36, 100, band="5G"), _ap(40, 100, band="5G"),     # 36 块 2.0
               _ap(52, 100, band="5G"),                              # 52 块 1.0+罚 1.0
               _ap(100, 100, band="5G"), _ap(104, 100, band="5G"),   # 100 块 2.0
               _ap(116, 100, band="5G"), _ap(120, 100, band="5G"),   # 116 块 2.0
               _ap(144, 100, band="5G"),                             # 132 块 1.0+罚 1.0
               _ap(149, 100, band="5G"), _ap(153, 100, band="5G")]   # 149 块 2.0
        rec = recommend_channels(aps)
        assert rec["5g"] == 36 and rec["5g_block"] == 2

    def test_channel_165_not_in_any_80m_block(self):
        """165 非标准 80M 块（模块头注释契约）：既不被推荐，也不给 149 块加占用。
        鉴别构造：36 块 1 个满强 AP（1.0）时，149 块为 0.0 应胜出——若 165 被
        错误划入 149 块，149 块变 1.0 与各空 DFS 块并列 → 回落荐 36，本用例红。"""
        rec = recommend_channels([_ap(165, 100, band="5G")])
        assert rec["5g"] == 36 and rec["5g_block"] == 0    # 全空环境：165 不被推荐
        rec2 = recommend_channels([_ap(36, 100, band="5G"), _ap(165, 100, band="5G")])
        assert rec2["5g"] == 149 and rec2["5g_block"] == 0  # 165 未污染 149 块

    def test_band_field_overrides_channel_derivation(self):
        """band 字段在场时优先于信道号推导（docstring 契约「缺省时」才推导）：
        ch6+band=5G 归 5G 侧（ch6 不属任何 5G 块→全空非 DFS 锚 36），
        2.4G 侧保持全空推荐 1。若反转为按信道优先，ch6 落 2.4G → 5g=None 即红。"""
        rec = recommend_channels([_ap(6, 100, band="5G")])
        assert rec["5g"] == 36 and rec["5g_block"] == 0
        assert rec["24g"] == 1 and rec["24g_same"] == 0 and rec["24g_adj"] == 0
