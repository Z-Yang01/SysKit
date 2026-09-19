"""syskit/secure_store.py：DPAPI 密封存储（透明迁移旧明文）。"""
import json

from syskit import secure_store as ss


class TestSealJson:
    def test_roundtrip(self, tmp_path):
        p = str(tmp_path / "sealed.json")
        assert ss.save_sealed_json(p, {"k": "值", "n": [1, 2]})
        data, sealed = ss.load_sealed_json(p, None)
        assert sealed is True
        assert data == {"k": "值", "n": [1, 2]}

    def test_container_shape(self, tmp_path):
        p = str(tmp_path / "s.json")
        ss.save_sealed_json(p, [1])
        outer = json.load(open(p, encoding="utf-8"))
        assert outer["enc"] == "dpapi" and outer["v"] == 1
        assert isinstance(outer["data"], str) and outer["data"]

    def test_legacy_plaintext_loaded_and_flagged(self, tmp_path):
        p = tmp_path / "legacy.json"
        p.write_text('[{"text": "old"}]', encoding="utf-8")
        data, sealed = ss.load_sealed_json(str(p), [])
        assert data == [{"text": "old"}] and sealed is False   # 兼容读取
        ss.save_sealed_json(str(p), data)                      # 下次保存自动密封
        again, sealed2 = ss.load_sealed_json(str(p), [])
        assert again == [{"text": "old"}] and sealed2 is True  # 已升级为密封


class TestCorruptAndEdge:
    def test_missing_file_returns_default(self, tmp_path):
        assert ss.load_sealed_json(str(tmp_path / "nope.json"), {"d": 1}) == ({"d": 1}, False)

    def test_sealed_by_other_user_or_corrupt_returns_default(self, tmp_path):
        p = str(tmp_path / "bad.json")
        ss.save_sealed_json(p, {"secret": 1})
        outer = json.load(open(p, encoding="utf-8"))
        outer["data"] = outer["data"][:-8] + "AAAAAA=="       # 破坏密文
        open(p, "w", encoding="utf-8").write(json.dumps(outer))
        assert ss.load_sealed_json(p, []) == ([], False)

    def test_save_creates_missing_dirs(self, tmp_path):
        p = tmp_path / "a" / "b" / "c.json"
        assert ss.save_sealed_json(str(p), {"x": 1}) is True
        assert p.exists()

    def test_atomic_no_tmp_left(self, tmp_path):
        p = str(tmp_path / "at.json")
        ss.save_sealed_json(p, {"a": 1})
        assert not (tmp_path / "at.json.tmp").exists()


class TestSealFailure:
    def test_save_returns_false_when_dpapi_unavailable(self, tmp_path, monkeypatch):
        """DPAPI 不可用（非 OSError 的 AttributeError 等）→ save 返回 False 不穿透。"""

        def boom(data):
            raise AttributeError("no windll on this platform")

        monkeypatch.setattr("syskit.secure_store._dpapi_protect", boom)
        assert ss.save_sealed_json(str(tmp_path / "x.json"), {"a": 1}) is False

    def test_save_failure_leaves_no_tmp(self, tmp_path, monkeypatch):

        monkeypatch.setattr("syskit.secure_store._dpapi_protect",
                            lambda data: (_ for _ in ()).throw(OSError("disk full")))
        p = str(tmp_path / "y.json")
        assert ss.save_sealed_json(p, {"a": 1}) is False
        assert not (tmp_path / "y.json.tmp").exists()
