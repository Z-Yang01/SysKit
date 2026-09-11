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
