"""syskit/clipboard.py 与 syskit/modes.py 单元测试（无真实注册表/进程/剪贴板）。"""
import json
import os


from syskit import clipboard as clip_mod
from syskit import config, modes
from syskit.proc import CmdResult

PNG_DAY = bytes([0x89]) + b'PNG-day'

PNG1 = bytes([0x89]) + b'PNG-fake-1'
PNG2 = bytes([0x89]) + b'PNG-fake-2'
PNG3 = bytes([0x89]) + b'PNG-fake-3'


# ---------------- ClipboardHistory ----------------

class TestClipboardHistory:
    def test_add_and_order_newest_first(self):
        h = clip_mod.ClipboardHistory(max_items=10)
        assert h.add("a")
        assert h.add("b")
        assert h.texts() == ["b", "a"]

    def test_blank_and_consecutive_dup_rejected(self):
        h = clip_mod.ClipboardHistory()
        assert h.add("  ") is False
        assert h.add("x")
        assert h.add("x") is False                    # 与最新一条相同
        assert h.add("y")
        assert h.add("x") is True                     # 非连续可再入
        assert h.texts() == ["x", "y", "x"]

    def test_max_items_trim(self):
        h = clip_mod.ClipboardHistory(max_items=3)
        for i in range(5):
            h.add(f"t{i}")
        assert h.texts() == ["t4", "t3", "t2"]

    def test_long_text_truncated(self):
        h = clip_mod.ClipboardHistory(max_len=64)
        h.add("Z" * 200)
        assert len(h.texts()[0]) == 64

    def test_persist_roundtrip(self, tmp_path, monkeypatch):
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        h1 = clip_mod.ClipboardHistory()
        h1.add("persist-me")
        h1.flush_if_dirty()                    # 防抖语义：显式刷盘
        h2 = clip_mod.ClipboardHistory()
        assert "persist-me" in h2.texts()

    def test_remove_and_clear(self):
        h = clip_mod.ClipboardHistory()
        h.add("a")
        h.add("b")
        assert h.remove(0)
        assert h.texts() == ["a"]
        assert h.remove(99) is False
        h.clear()
        assert h.items() == []

    def test_preview(self):
        assert clip_mod.ClipboardHistory.preview("hello\nworld") == "hello world"
        assert clip_mod.ClipboardHistory.preview("x" * 100, n=10).endswith("…")
        assert len(clip_mod.ClipboardHistory.preview("x" * 100, n=10)) == 10

    # ---------------- 新特性：kind / 置顶 / 搜索 / 敏感 ----------------

    def test_clipboard_file_encrypted_at_rest(self, tmp_path, monkeypatch):
        """回归：clipboard.json 落盘为 DPAPI 密封容器，磁盘上不含明文历史。"""
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        h = clip_mod.ClipboardHistory()
        h.add("secret-clipboard-content")
        h.flush_if_dirty()                     # 防抖语义：显式刷盘
        raw = (tmp_path / "clipboard.json").read_text(encoding="utf-8")
        assert "secret-clipboard-content" not in raw
        outer = json.loads(raw)
        assert outer["enc"] == "dpapi"
        # 重启（新实例）仍可读回
        h2 = clip_mod.ClipboardHistory()
        assert h2.texts() == ["secret-clipboard-content"]

    def test_kind_entry_roundtrip(self, tmp_path, monkeypatch):
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        h1 = clip_mod.ClipboardHistory()
        assert h1.add("C:\\a\\b.txt", kind="file")
        assert h1.add("[图片 4×4]", kind="image", data="abc.png")
        h1.flush_if_dirty()                    # 防抖语义：显式刷盘
        h2 = clip_mod.ClipboardHistory()
        kinds = {it["text"]: it["kind"] for it in h2.items()}
        assert kinds["C:\\a\\b.txt"] == "file"
        assert kinds["[图片 4×4]"] == "image"
        assert [it["data"] for it in h2.items() if it["kind"] == "image"] == ["abc.png"]

    def test_old_format_json_compat(self, tmp_path, monkeypatch):
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        old = [{"ts": 456, "text": "n2", "kind": "html"}, {"ts": 123, "text": "legacy"}]
        (tmp_path / "clipboard.json").write_text(json.dumps(old), encoding="utf-8")
        h = clip_mod.ClipboardHistory()
        assert h.texts() == ["n2", "legacy"]
        first = h.items()[0]
        assert first["kind"] == "html" and first["pinned"] is False
        assert h.items()[1]["kind"] == "text"

    def test_pin_toggle_resort(self):
        h = clip_mod.ClipboardHistory()
        for t in ("a", "b", "c"):
            h.add(t)
        for i, it in enumerate(h._items):           # 显式时间戳避免同秒并列
            it["ts"] = 300 - i                      # c=300, b=299, a=298
        assert h.texts() == ["c", "b", "a"]
        assert h.toggle_pin(2)                      # 置顶最老的 a
        assert [it["pinned"] for it in h.items()] == [True, False, False]
        assert h.texts() == ["a", "c", "b"]
        assert h.toggle_pin(0)
        assert h.texts() == ["c", "b", "a"]
        assert h.toggle_pin(99) is False

    def test_pinned_not_evicted(self):
        h = clip_mod.ClipboardHistory(max_items=2)
        h.add("keep")
        h.add("m1")
        h.toggle_pin(1)                             # keep → [keep, m1]
        h.add("m2")                                 # 非置顶 m1,m2 未超上限
        assert h.texts() == ["keep", "m2", "m1"]
        h.add("m3")                                 # 非置顶超额 → 淘汰最老 m1
        assert h.texts() == ["keep", "m3", "m2"]

    def test_pinned_dup_add_rejected(self):
        h = clip_mod.ClipboardHistory()
        h.add("x")
        h.toggle_pin(0)
        h.add("y")
        assert h.add("x") is False                  # 与置顶条目相同 → 不入库
        assert h.texts() == ["x", "y"]              # 置顶块始终在前
        # 图片条目：描述相同但 data 不同仍可入库
        assert h.add("[图片 4×4]", kind="image", data="a.png")
        assert h.add("[图片 4×4]", kind="image", data="a.png") is False
        assert h.add("[图片 4×4]", kind="image", data="b.png") is True

    def test_search_keyword_and_kind(self):
        h = clip_mod.ClipboardHistory()
        h.add("Alpha one")
        h.add("beta")
        h.add("gamma ALPHA", kind="html", data="<i>x</i>")
        hits = h.search("alpha")
        assert [it["text"] for it in hits] == ["gamma ALPHA", "Alpha one"]
        assert hits[1]["idx"] == 2                  # idx 映射原始索引
        only_html = h.search(None, kind="html")
        assert [it["text"] for it in only_html] == ["gamma ALPHA"]
        assert h.search("不存在的词") == []

    def test_sensitive_skip(self, monkeypatch):
        clip_mod._sensitive.clear()
        try:
            clip_mod.mark_sensitive("secret-token", ttl=30)
            h = clip_mod.ClipboardHistory()
            assert h.add("secret-token") is False
            assert h.items() == []
            assert h.add("other") is True
            # 过期后不再拦截
            clip_mod._sensitive[:] = [{"digest": list(clip_mod._sensitive)[0]["digest"],
                                       "until": 0}]
            assert h.add("secret-token") is True
        finally:
            clip_mod._sensitive.clear()

    def test_should_record_now_blacklist(self, monkeypatch):
        monkeypatch.setattr(clip_mod, "foreground_process_name",
                            lambda: "keepass.exe")
        assert clip_mod.should_record_now(blacklist="keepass, other") is False
        assert clip_mod.should_record_now(blacklist="") is True
        monkeypatch.setattr(clip_mod, "foreground_process_name", lambda: "notepad")
        assert clip_mod.should_record_now(blacklist="KEEPASS.EXE,bitwarden") is True

    def test_purge_skips_entries_without_timestamp(self, tmp_path, monkeypatch):
        """回归：无 ts 的旧版本条目（ts=0）不得被按天清理误删（宁留勿误删）。"""
        import time
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        h = clip_mod.ClipboardHistory()
        now = time.time()
        h._items = [
            {"ts": 0, "text": "无时间戳旧条目", "kind": "text", "pinned": False, "data": ""},
            {"ts": int(now - 5 * 86400), "text": "真过期", "kind": "text", "pinned": False, "data": ""},
        ]
        assert h.purge_older_than(1) == 1
        assert h.texts() == ["无时间戳旧条目"]     # 无时效数据保留
        assert h.texts() != ["真过期"]

    def test_purge_older_than_daily_semantics(self, tmp_path, monkeypatch):
        """按天过期：过期条目删除（含图片载荷与置顶条目），未过期保留；0=永不过期。"""
        import time
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        h = clip_mod.ClipboardHistory()
        name = clip_mod.save_image_bytes(PNG_DAY)
        now = time.time()
        h._items = [
            {"ts": int(now - 3 * 86400), "text": "三天前", "kind": "text", "pinned": False, "data": ""},
            {"ts": int(now - 2 * 86400), "text": "两天前图", "kind": "image", "pinned": True, "data": name},
            {"ts": int(now - 3600), "text": "一小时前", "kind": "text", "pinned": False, "data": ""},
            {"ts": int(now), "text": "刚刚", "kind": "text", "pinned": False, "data": ""},
        ]
        removed = h.purge_older_than(1)             # 一天一删
        assert removed == 2                          # 三天前 + 两天前图（置顶同样过期）
        texts = h.texts()
        assert set(texts) == {"刚刚", "一小时前"}    # 过期删除，未过期保留
        assert not os.path.exists(clip_mod.image_path(name))   # 图片载荷同删
        assert h._dirty                              # 已标脏（待定时器刷盘）
        # 0 = 永不过期
        assert h.purge_older_than(0) == 0
        assert len(h.items()) == 2

    def test_image_payload_removed_on_remove_clear_evict(self, tmp_path, monkeypatch):
        """回归：删除/清空/超额淘汰图片条目时，明文 PNG 载荷同步删除（不留磁盘残留）。"""
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        h = clip_mod.ClipboardHistory(max_items=3)
        png = PNG1
        name = clip_mod.save_image_bytes(png)
        h.add("[图片 1×1]", kind="image", data=name)
        path = clip_mod.image_path(name)
        assert os.path.exists(path)
        h.remove(0)                                   # 单条删除 → PNG 同删
        assert not os.path.exists(path)
        name2 = clip_mod.save_image_bytes(PNG2)
        h.add("[图片 2×2]", kind="image", data=name2)
        h.clear()                                     # 清空 → PNG 同删
        assert not os.path.exists(clip_mod.image_path(name2))
        # 超额淘汰：4 条挤掉最老图片
        name3 = clip_mod.save_image_bytes(PNG3)
        h.add("[图片 3×3]", kind="image", data=name3)
        for i in range(3):
            h.add(f"t{i}")
        assert not os.path.exists(clip_mod.image_path(name3))   # 最老被淘汰
        h.clear()

    def test_save_image_bytes(self, tmp_path, monkeypatch):
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        assert clip_mod.save_image_bytes(b"") == ""
        name = clip_mod.save_image_bytes(b"\x89PNG-fake")
        assert name.endswith(".png")
        assert (tmp_path / "clipboard_files" / name).read_bytes() == b"\x89PNG-fake"
        assert clip_mod.save_image_bytes(b"\x89PNG-fake") == name    # md5 去重同名
        assert clip_mod.image_path(name) == str(tmp_path / "clipboard_files" / name)


# ---------------- modes helpers ----------------

def _ok(name="x"):
    return modes._step(name, True, "ok")


def _fail(name="x"):
    return modes._step(name, False, "err")


class TestModeSteps:
    # （test_set_notifications_success 已删除：曾替换被测函数自身后断言 fake 的
    #   硬编码返回值，恒真零检出；真本体注册表路径由 test_charts_modes_gaps
    #   的假 winreg 用例覆盖。）

    def test_set_mute_uses_ps(self, monkeypatch):
        seen = []

        def fake_ps(script, timeout=40):
            seen.append(script)
            if "waveOutGetVolume" in script:
                return CmdResult(0, "1073741823\n", "")      # 模拟当前音量 25%
            return CmdResult(0, "", "")

        monkeypatch.setattr(modes.proc, "run_ps", fake_ps)
        monkeypatch.setattr("syskit.settings.save", lambda patch: None)
        monkeypatch.setattr("syskit.settings.load", lambda: {})
        r = modes.set_mute(True)
        assert r["ok"] and "已静音" in r["detail"]
        assert any("waveOutGetVolume" in s for s in seen)    # 静音前先读原音量
        assert any("waveOutSetVolume" in s and "[uint32]0" in s for s in seen)

    def test_unmute_restores_previous_volume(self, monkeypatch):
        """回归：取消静音必须恢复进模式前的音量，而不是置最大音量
        （旧行为把用户 30% 的音量变成 100%）。"""
        seen = []

        def fake_ps(script, timeout=40):
            seen.append(script)
            return CmdResult(0, "", "")

        monkeypatch.setattr(modes.proc, "run_ps", fake_ps)
        monkeypatch.setattr("syskit.settings.load",
                            lambda: {"modes_prev_volume": 805306368})  # ≈50%
        r = modes.set_mute(False)
        assert r["ok"] and "已恢复原音量" in r["detail"]
        assert any("805306368" in s for s in seen)           # 恢复的是记录值
        assert not any("0xFFFFFFFF" in s for s in seen)

    def test_unmute_without_record_sets_max(self, monkeypatch):
        # 无记录（非本工具做的静音）→ 保持旧行为置最大，并注明
        monkeypatch.setattr(modes.proc, "run_ps",
                            lambda script, timeout=40: CmdResult(0, "", ""))
        monkeypatch.setattr("syskit.settings.load", lambda: {})
        r = modes.set_mute(False)
        assert r["ok"] and "未找到" in r["detail"]

    def test_set_mute_fail(self, monkeypatch):
        monkeypatch.setattr(modes.proc, "run_ps",
                            lambda *a, **k: CmdResult(1, "", "boom"))
        monkeypatch.setattr("syskit.settings.load", lambda: {})
        r = modes.set_mute(False)
        assert r["ok"] is False

    def test_open_common_apps(self, monkeypatch):
        opened = []
        monkeypatch.setattr(modes.os, "startfile", lambda t: opened.append(t), raising=False)
        # os.startfile 在非 win32 不存在时 monkeypatch 到 modes 模块引用的 os
        steps = modes.open_common_apps(["记事本", "未知"])
        assert steps[0]["ok"] and "notepad" in steps[0]["detail"]
        assert steps[1]["ok"] is False

    def test_backup_config_copies_files(self, tmp_path, monkeypatch):
        appdata = tmp_path / "appdata"
        backup = tmp_path / "backup"
        monkeypatch.setattr(config, "APPDATA_DIR", str(appdata), raising=False)
        monkeypatch.setattr(config, "BACKUP_DIR", str(backup), raising=False)
        appdata.mkdir(parents=True)
        (appdata / "settings.json").write_text('{"a":1}', encoding="utf-8")
        (appdata / "traffic.json").write_text("{}", encoding="utf-8")
        r = modes.backup_config()
        assert r["ok"]
        # 备份目录下应有子目录
        kids = list(backup.iterdir())
        assert len(kids) == 1 and (kids[0] / "settings.json").is_file()

    def test_backup_config_empty_still_succeeds(self, tmp_path, monkeypatch):
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path / "empty"), raising=False)
        monkeypatch.setattr(config, "BACKUP_DIR", str(tmp_path / "b2"), raising=False)
        from syskit import settings as settings_mod
        monkeypatch.setattr(settings_mod, "load", lambda: {"dark_mode": True})
        r = modes.backup_config()
        assert r["ok"]

    def test_set_power_plan(self, monkeypatch):
        seen = {}

        def fake_run(cmd, timeout=20):
            seen["cmd"] = list(cmd)
            return CmdResult(0, "", "")

        monkeypatch.setattr(modes.proc, "run", fake_run)
        r = modes.set_power_plan(True)
        assert r["ok"] and seen["cmd"] == ["powercfg", "/setactive", "SCHEME_MIN"]   # SCHEME_MIN=高性能（别名语义）
        r = modes.set_power_plan(False)
        assert seen["cmd"][-1] == "SCHEME_BALANCED"

    def test_pause_update_requires_admin(self, monkeypatch):
        monkeypatch.setattr(modes.admin, "is_admin", lambda: False)
        r = modes.pause_windows_update(7)
        assert r["ok"] is False and "管理员" in r["detail"]

    def test_pause_update_as_admin(self, monkeypatch):
        writes = []
        monkeypatch.setattr(modes.admin, "is_admin", lambda: True)

        class FakeKey:
            def Close(self):
                pass

        class FakeWinreg:
            HKEY_LOCAL_MACHINE = 1
            KEY_SET_VALUE = 2
            REG_DWORD = 4

            @staticmethod
            def OpenKey(*a, **k):
                raise OSError("missing")

            @staticmethod
            def CreateKeyEx(*a, **k):
                return FakeKey()

            @staticmethod
            def SetValueEx(key, name, reserved, typ, value):
                writes.append((name, typ, value))

        import sys
        monkeypatch.setitem(sys.modules, "winreg", FakeWinreg)
        r = modes.pause_windows_update(7)
        assert r["ok"] is True, r
        assert writes and writes[0][0] == "NoAutoUpdate" and writes[0][2] == 1

    def test_close_background_empty(self, monkeypatch):
        monkeypatch.setattr(modes, "list_background_candidates", lambda: [])
        r = modes.close_background([])
        assert r["ok"] and "无需清理" in r["detail"]

    def test_close_background_kills_whitelist_only(self, monkeypatch):
        killed = []

        class FakeProc:
            def __init__(self, pid):
                self.pid = pid

            def name(self):
                return "Spotify.exe"

            def kill(self):
                killed.append(self.pid)

        monkeypatch.setattr(
            "psutil.Process", lambda pid: FakeProc(pid), raising=False)
        # 直接注入 targets，避免真实 psutil 扫描
        import syskit.modes as m
        r = m.close_background([{"pid": 123, "name": "Spotify.exe"}])
        assert r["ok"] and killed == [123]

        # PID 复用防护：kill 前复验进程名——名字不匹配（PID 被无关进程复用）→ 跳过不杀
        class RenamedProc(FakeProc):
            def name(self):
                return "explorer.exe"

        monkeypatch.setattr("psutil.Process", lambda pid: RenamedProc(pid), raising=False)
        r2 = m.close_background([{"pid": 123, "name": "Spotify.exe"}])
        assert killed == [123]                    # 未新增击杀
        assert r2["ok"] is False                  # 复用防护跳过后不得报成功
        # modes.py 只把失败计数写进 detail（"失败 1"），跳过原因仅存在于内部 failed 列表
        assert "失败 1" in r2["detail"]


    def test_run_office_mode_order(self, monkeypatch):
        order = []

        def track(fn, name):
            def wrap(*a, **k):
                order.append(name)
                return _ok(name)
            return wrap

        monkeypatch.setattr(modes, "set_notifications",
                            track(None, "notify"))
        monkeypatch.setattr(modes, "set_mute", track(None, "mute"))
        monkeypatch.setattr(modes, "open_common_apps",
                            lambda names=None: (order.append("apps"), [_ok("打开")])[-1])
        monkeypatch.setattr(modes, "backup_config",
                            track(None, "backup"))
        steps = modes.run_office_mode(open_apps=True)
        assert order == ["notify", "mute", "apps", "backup"]
        assert len(steps) == 4

    def test_run_game_mode_order(self, monkeypatch):
        order = []

        def mk(name):
            def _f(*a, **k):
                order.append(name)
                return _ok(name)
            return _f

        monkeypatch.setattr(modes, "set_notifications", mk("notify"))
        monkeypatch.setattr(modes, "set_power_plan", mk("power"))
        monkeypatch.setattr(modes, "pause_windows_update", mk("wu"))
        monkeypatch.setattr(modes, "close_background",
                            lambda processes=None: (order.append("bg"), _ok("bg"))[-1])
        steps = modes.run_game_mode(clear_bg=True)
        assert order == ["notify", "power", "wu", "bg"]
        assert len(steps) == 4

    def test_restore_functions_call_restores(self, monkeypatch):
        monkeypatch.setattr(modes, "set_notifications", lambda en: _ok())
        monkeypatch.setattr(modes, "set_mute", lambda m: _ok())
        monkeypatch.setattr(modes, "set_power_plan", lambda h: _ok())
        monkeypatch.setattr(modes, "pause_windows_update", lambda d: _ok())
        assert len(modes.restore_office_defaults()) == 2
        assert len(modes.restore_game_defaults()) == 3


def test_flush_pending_closes_debounce_window(tmp_path, monkeypatch):
    """托盘/面板读磁盘态前调 flush_pending：10s 防抖窗口内最新复制可见。"""
    assert "flush-me" not in clip_mod.ClipboardHistory(max_items=5).texts()
    h1 = clip_mod.ClipboardHistory(max_items=5)
    assert h1.add("flush-me") is True
    # 另一实例读磁盘旧态（replica of tray/palette 的读法）
    assert "flush-me" not in clip_mod.ClipboardHistory(max_items=5).texts()
    assert clip_mod.flush_pending() >= 1   # 未落盘实例被刷盘
    assert "flush-me" in clip_mod.ClipboardHistory(max_items=5).texts()
