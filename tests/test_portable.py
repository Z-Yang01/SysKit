"""便携数据模式：exe 旁存在 data 目录时，全部数据路径收敛到安装目录。"""
import importlib
import sys


def test_portable_data_dirs(tmp_path, monkeypatch):
    from syskit import config as cfg

    app = tmp_path / "app"
    (app / "data").mkdir(parents=True)
    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.setattr(sys, "executable", str(app / "SysKit.exe"))

    try:
        cfg2 = importlib.reload(cfg)
        assert cfg2._PORTABLE == str(app / "data")
        assert cfg2.APPDATA_DIR == str(app / "data" / "config")
        assert cfg2.LOCALAPPDATA_DIR == str(app / "data" / "local")
        assert cfg2.SETTINGS_FILE == str(app / "data" / "config" / "settings.json")
        assert cfg2.LOG_DIR == str(app / "data" / "local" / "logs")
        assert cfg2.BACKUP_DIR == str(app / "data" / "local" / "backup")
    finally:
        monkeypatch.undo()
        importlib.reload(cfg)                    # 恢复全局 config 状态


def test_normal_data_dirs_when_no_marker():
    """无 data 目录（或源码运行）→ 保持 %APPDATA% 布局。"""
    from syskit import config as cfg

    assert cfg._portable_data_dir() == ""        # 项目根无 data 目录
    assert cfg.SETTINGS_FILE.startswith(cfg.APPDATA_DIR)
    assert cfg.BACKUP_DIR.startswith(cfg.LOCALAPPDATA_DIR)


def test_non_portable_warns_on_leftover_portable_data(tmp_path, monkeypatch):
    """回归：便携→非便携切换后，检测到安装目录 data\ 有历史数据且当前目录全新
    → 必须记日志指明数据位置（防用户误以为丢失）；不自动回迁。"""
    import os

    from syskit import config as cfg
    from syskit.ui import app as app_mod

    app = tmp_path / "app"
    data_cfg = app / "data" / "config"
    data_cfg.mkdir(parents=True)
    (data_cfg / "vault.db").write_text("db", encoding="utf-8")
    current = tmp_path / "cur"          # 当前 %APPDATA%：全新（无 settings.json）

    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.setattr(sys, "executable", str(app / "SysKit.exe"))
    monkeypatch.setattr(cfg, "_PORTABLE", "", raising=False)
    monkeypatch.setattr(cfg, "APPDATA_DIR", str(current), raising=False)
    monkeypatch.setattr(cfg, "LOCALAPPDATA_DIR", str(tmp_path / "curlocal"), raising=False)

    warnings_log = []
    monkeypatch.setattr("syskit.logger.LOGGER.warning",
                        lambda msg, *a, **k: warnings_log.append(msg.format(*a, **k)))

    app_mod._prepare_data_dirs()
    assert len(warnings_log) == 1
    assert "数据" in warnings_log[0] and str(data_cfg) in warnings_log[0]
    # 当前目录已建好但不写任何迁移数据
    assert os.path.isdir(current)

    # 当前目录已有 settings.json（非全新）→ 不提示
    warnings_log.clear()
    (current / "settings.json").write_text("{}", encoding="utf-8")
    app_mod._prepare_data_dirs()
    assert warnings_log == []


def test_vault_connect_creates_missing_dirs(tmp_path, monkeypatch):
    """回归：便携首启 data\config 不存在时，密码库自动建目录而不是报
    unable to open database file。"""
    from syskit import vault

    nested = tmp_path / "fresh" / "config"       # 故意不存在
    monkeypatch.setattr(vault.config, "APPDATA_DIR", str(nested), raising=False)
    vault.init_db()
    sid = vault.add_site("GitHub")
    assert (nested / "vault.db").exists()
    assert vault.search_sites("GitHub")[0]["name"] == "GitHub"
    _ = sid


def test_prepare_data_dirs_migrates_old_appdata(tmp_path, monkeypatch):
    """便携首启迁移：旧 %APPDATA% 数据 → data\config，不覆盖已有新数据。"""
    import os

    from syskit import config as cfg
    from syskit.ui import app as app_mod

    old = tmp_path / "old"
    new = tmp_path / "new"
    (old / "clipboard_files").mkdir(parents=True)
    (old / "settings.json").write_text('{"dark_mode": true}', encoding="utf-8")
    (old / "vault.db").write_text("db-bytes", encoding="utf-8")
    (old / "clipboard_files" / "x.png").write_text("png", encoding="utf-8")

    monkeypatch.setattr(cfg, "_PORTABLE", str(tmp_path / "portable"), raising=False)
    monkeypatch.setattr(cfg, "_APP_DATA_ROOT", str(old), raising=False)
    monkeypatch.setattr(cfg, "APPDATA_DIR", str(new), raising=False)
    monkeypatch.setattr(cfg, "LOCALAPPDATA_DIR", str(tmp_path / "newlocal"), raising=False)

    app_mod._prepare_data_dirs()
    assert (new / "settings.json").read_text(encoding="utf-8") == '{"dark_mode": true}'
    assert (new / "vault.db").read_text(encoding="utf-8") == "db-bytes"
    assert (new / "clipboard_files" / "x.png").read_text(encoding="utf-8") == "png"

    # 新目录已有数据时不得覆盖
    (new / "settings.json").write_text("{}", encoding="utf-8")
    (old / "settings.json").write_text('{"dark_mode": false}', encoding="utf-8")
    os.remove(new / "vault.db")
    app_mod._prepare_data_dirs()
    assert (new / "settings.json").read_text(encoding="utf-8") == "{}"
    assert not (new / "vault.db").exists()
