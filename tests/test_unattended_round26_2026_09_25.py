"""无人值守轮次26（2026-09-25）逻辑层 bug 猎手回归锁。

四处实锤修复的防回归（每例注明：锁被删改后实现若回退，哪个行为特征会漏网）：

1. settings 类型漂移（合法 JSON + 错误类型，如手改 poll_interval_sec="很快"）
   此前原样穿透 load/save → monitor.settings() 的 int() 位于 run() 循环 try
   之外，一个坏键即让监控线程静默死亡；vault_page/palette/clip_paste/tray 等
   处的 int() 同样裸奔 → load/save 双侧按 DEFAULTS 类型收敛（_typed）。
2. TUNNEL_PAT 的 tap9 是死令牌（真实描述 "TAP-Win32 Adapter V9" 里 V9 与 TAP
   之间隔着整段文本，连续子串 "tap9" 永不命中）→ 旧版 OpenVPN 驱动在用时
   should_skip_auto_repair 放行 renew_dhcp/restart_wlan/toggle_adapter，恰好
   砍断 OpenVPN 会话 → 补 tap-win32 令牌。
3. del_persistent_route 只填 dest 框时，原实现落入「自动检测默认路由」分支 →
   用户想删 10.0.0.0/8，实际删掉的却是 0.0.0.0/0 → 改为按 dest 反查持久路由
   网关；查不到明确拒绝且不执行任何删除。
4. diskusage 扫描中单个文件 stat 失败只计全局 error_count、不计目录节点 →
   ScanResult 与 DirNode（父目录汇总）口径分叉 → 两处同计。
"""
import json
import os

import pytest

from syskit import config, repairs, settings as settings_mod
from syskit.monitor import Monitor
from syskit.repairs import RepairResult
from test_repairs import Recorder, _patch_snapshot, _snap


# ---------------- 工具 ----------------

@pytest.fixture
def recorder(monkeypatch, fake_cmd_result):
    """conftest 哨兵的显式覆盖：proc.run/run_ps 换成记录器（不真实执行）。"""
    from syskit import proc
    rec = Recorder(fake_cmd_result, [])
    monkeypatch.setattr(proc, "run", rec.run)
    monkeypatch.setattr(proc, "run_ps", rec.run_ps)
    return rec

def _write_settings(payload):
    """conftest.isolated_appdata 已把 SETTINGS_FILE 重定向到 pytest 临时目录。"""
    os.makedirs(config.APPDATA_DIR, exist_ok=True)
    with open(config.SETTINGS_FILE, "w", encoding="utf-8") as f:
        json.dump(payload, f, ensure_ascii=False)
    settings_mod._cache = None


class _NoWaitEvent:
    """替身 Event：_run_ladder 步间 _stop_evt.wait(3) 立即返回（不真睡 3s）。"""

    def is_set(self):
        return False

    def wait(self, timeout=None):
        return False


# ---------------- 修复1：settings 类型漂移收敛 ----------------

class TestSettingsTypeDrift:
    def test_drifted_int_keys_fall_back_and_monitor_settings_survives(self):
        """监控线程生死锁：修复前 Monitor.settings() 对 "很快" 抛 ValueError，
        而该调用在 monitor.run() 的 try 之外 → 线程死亡、监控静默停摆。
        变异验证：删 _typed 调用后本例首个断言即抛 ValueError。"""
        _write_settings({"poll_interval_sec": "很快", "fail_threshold": [2]})
        st = Monitor().settings()          # 修复前：ValueError
        assert st["poll_interval_sec"] == config.DEFAULTS["poll_interval_sec"]
        assert st["fail_threshold"] == config.DEFAULTS["fail_threshold"]

    def test_digit_string_float_and_bool_int_coerce(self):
        """可收敛的漂移按意图收敛（不静默丢用户的手改）。"""
        _write_settings({"clipboard_max": "50", "cooldown_sec": 180.0,
                         "auto_repair_enabled": 0, "fail_threshold": True})
        data = settings_mod.load()
        assert data["clipboard_max"] == 50
        assert data["cooldown_sec"] == 180
        assert data["auto_repair_enabled"] is False
        assert data["fail_threshold"] == 1

    def test_str_key_rejects_number_and_bool(self):
        """str 键收到数字/None 视为损坏：回退默认，而不是把 5 当预设名。"""
        _write_settings({"dns_preset": 5, "palette_hotkey": None})
        data = settings_mod.load()
        assert data["dns_preset"] == config.DEFAULTS["dns_preset"]
        assert data["palette_hotkey"] == config.DEFAULTS["palette_hotkey"]

    def test_unknown_keys_pass_through(self):
        """收敛只作用于 DEFAULTS 已知键：last_page / modes_prev_volume 等动态键
        原样保留（否则 last_page 丢失、静音恢复失效）。"""
        _write_settings({"last_page": 3, "modes_prev_volume": 4294967295})
        data = settings_mod.load()
        assert data["last_page"] == 3
        assert data["modes_prev_volume"] == 4294967295

    def test_save_patch_coerced_before_cache_and_disk(self):
        """补丁同过收敛：坏值既不进缓存也不落盘。变异验证：save 去掉 _typed 后
        磁盘上 dark_mode 会是整数 1 而非 true。"""
        settings_mod.save({"dark_mode": 1, "poll_interval_sec": "45"})
        assert settings_mod.load()["dark_mode"] is True
        assert settings_mod.load()["poll_interval_sec"] == 45
        with open(config.SETTINGS_FILE, encoding="utf-8") as f:
            on_disk = json.load(f)
        assert on_disk["dark_mode"] is True
        assert on_disk["poll_interval_sec"] == 45


# ---------------- 修复2：TUNNEL_PAT 补 tap-win32（护栏假阴性） ----------------

class TestTunnelPatTapWin32:
    def test_openvpn_tap_family_descs_matched(self):
        """变异验证：删掉 tap-win32 令牌后首例（TAP-Win32 Adapter V9）即失败。"""
        from syskit.vpn import TUNNEL_PAT
        for desc in ("TAP-Win32 Adapter V9",
                     "Netease UU TAP-Win32 Adapter V9.21",
                     "TAP-Windows Adapter V9"):
            assert TUNNEL_PAT.search(desc), desc

    def test_non_vpn_adapters_still_ignored(self):
        """误报面不扩大：常见物理/虚拟网卡与过渡技术伪适配器不因本次改动命中。"""
        from syskit.vpn import TUNNEL_PAT
        for desc in ("Realtek PCIe GbE Family Controller",
                     "Intel(R) Wi-Fi 6 AX201 160MHz",
                     "VMware Virtual Ethernet Adapter for VMnet8",
                     "WAN Miniport (IP)",
                     "Bluetooth Device (Personal Area Network)"):
            assert not TUNNEL_PAT.search(desc), desc

    def test_tap_win32_guard_skips_l3_ladder(self, monkeypatch):
        """护栏语义锁：TAP-Win32 在用时 L3 梯度只保留无高危动作。变异验证：
        tap-win32 令牌删除后 executed 会混入 renew_dhcp/restart_wlan。"""
        from syskit import monitor as monitor_mod, snapshot
        from syskit.snapshot import Adapter, Snapshot
        s = Snapshot(ok=True)
        s.adapters = [Adapter(name="以太网 2", desc="TAP-Win32 Adapter V9",
                              status="Up", if_index=11, ipv4=["10.3.0.2"])]
        monkeypatch.setattr(snapshot, "take", lambda use_ps=True: s)
        used = []

        class _FakeAct:
            title = "fake"

            def __init__(self, fid):
                self.id = fid

            def execute(self, param):
                used.append(self.id)
                return RepairResult(True, "ok")

        monkeypatch.setattr("syskit.repairs.get_action", lambda fid: _FakeAct(fid))
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda timeout=4.0: False)
        m = Monitor()
        m._stop_evt = _NoWaitEvent()
        m._run_ladder(3)
        assert used == ["flush_dns", "start_nlasvc"]    # 高危三项被护栏拦下


# ---------------- 修复3：del_persistent_route 只传 dest 时误删默认路由 ----------------

class TestDelPersistentRouteDestOnly:
    def test_dest_only_looks_up_gateway_from_persistent_routes(self, recorder, monkeypatch):
        """变异验证：回退旧实现（dest-only 落自动检测分支）后，本例会看到
        route delete 0.0.0.0 —— 用户要删的 10.0.0.0/8 反而没删。"""
        _patch_snapshot(
            monkeypatch,
            _snap(persistent=[{"dest": "10.0.0.0/8", "nextHop": "172.16.2.1", "metric": 5},
                              {"dest": "0.0.0.0/0", "nextHop": "172.16.2.1", "metric": 1}],
                  defaults=[dict(nextHop="172.16.2.1")]),
            _snap())
        r = repairs.get_action("del_persistent_route").execute({"dest": "10.0.0.0/8"})
        assert r.success is True
        assert recorder.find("route delete 10.0.0.0 mask 255.0.0.0 172.16.2.1") is not None
        # 绝不许碰默认路由
        assert recorder.find("route delete 0.0.0.0") is None
        assert "Remove-NetRoute -DestinationPrefix 10.0.0.0/8" in recorder.ps_scripts[0]
        # 备份里的还原命令携带快照真实跃点（5），而非合成值 1
        from pathlib import Path
        notes = list(Path(config.BACKUP_DIR).rglob("route_backup.txt"))
        assert notes and "metric 5" in notes[0].read_text(encoding="utf-8")

    def test_dest_only_unknown_dest_refuses_without_deleting(self, recorder, monkeypatch):
        """dest 在持久路由中查无此网段：明确拒绝，不执行任何删除命令。"""
        _patch_snapshot(
            monkeypatch,
            _snap(persistent=[{"dest": "0.0.0.0/0", "nextHop": "172.16.2.1", "metric": 1}]))
        r = repairs.get_action("del_persistent_route").execute({"dest": "192.168.50.0/24"})
        assert r.success is False
        assert "未找到" in r.message
        assert recorder.calls == []

    def test_dest_and_gw_still_synthetic_metric(self, recorder, monkeypatch):
        """显式 dest+gw 的既有契约不回归（合成 metric=1，掩码按前缀换算）。"""
        _patch_snapshot(monkeypatch, _snap(), _snap())
        r = repairs.get_action("del_persistent_route").execute(
            {"dest": "10.0.0.0/24", "gw": "10.0.0.1"})
        assert r.success is True
        assert recorder.find("route delete 10.0.0.0 mask 255.255.255.0 10.0.0.1") is not None


# ---------------- 修复4：diskusage stat 失败的双口径一致 ----------------

class _BadEntry:
    """stat 必失败的伪 DirEntry（模拟权限拒绝/文件被夺走）。"""

    def __init__(self, ent):
        self.name = ent.name
        self.path = ent.path

    def stat(self, follow_symlinks=False):
        raise OSError("模拟 stat 失败（权限拒绝）")


class TestScanTreeStatFailureCounting:
    def test_stat_failure_counted_on_node_and_total(self, monkeypatch, tmp_path):
        """变异验证：去掉 node.error_count += 1 后 res.root.error_count 退回 0，
        ScanResult 与 DirNode 汇总口径再度分叉。"""
        root = tmp_path / "t"
        root.mkdir()
        (root / "ok.txt").write_bytes(b"x" * 100)
        (root / "locked.bin").write_bytes(b"y" * 200)

        from syskit import diskusage as du
        real_scandir = os.scandir

        def fake_scandir(path):
            it = real_scandir(path)

            def gen():
                for ent in it:
                    yield _BadEntry(ent) if ent.name == "locked.bin" else ent
            return gen()

        monkeypatch.setattr(du.os, "scandir", fake_scandir)
        res = du.scan_tree(str(root))
        assert res.error_count == 1
        assert res.root.error_count == 1      # 修复前：节点侧 0（与全局口径分叉）
        assert res.root.file_count == 1
        assert res.root.total == 100          # 幸存文件照常计数
