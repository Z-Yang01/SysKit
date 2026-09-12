# -*- mode: python ; coding: utf-8 -*-
# PyInstaller 配置：单文件 + 管理员清单 + 无控制台 + 裁剪 Qt 重型模块

block_cipher = None

QT_HEAVY = [
    "PySide6.QtWebEngineCore", "PySide6.QtWebEngineWidgets", "PySide6.QtWebChannel",
    "PySide6.QtQuick", "PySide6.QtQuick3D", "PySide6.QtQuickWidgets", "PySide6.QtQuickControls2",
    "PySide6.QtQml", "PySide6.QtQmlModels", "PySide6.QtMultimedia", "PySide6.QtMultimediaWidgets",
    "PySide6.Qt3DCore", "PySide6.Qt3DRender", "PySide6.Qt3DInput", "PySide6.Qt3DLogic",
    "PySide6.Qt3DAnimation", "PySide6.Qt3DExtras", "PySide6.QtCharts", "PySide6.QtDataVisualization",
    "PySide6.QtPdf", "PySide6.QtPdfWidgets", "PySide6.QtRemoteObjects", "PySide6.QtScxml",
    "PySide6.QtSensors", "PySide6.QtSerialPort", "PySide6.QtSql", "PySide6.QtTest",
    "PySide6.QtTextToSpeech", "PySide6.QtVirtualKeyboard", "PySide6.QtWebSockets",
    "PySide6.QtDesigner", "PySide6.QtBluetooth", "PySide6.QtNfc", "PySide6.QtPositioning",
    "PySide6.QtLocation", "PySide6.QtHelp", "PySide6.QtUiTools", "PySide6.QtOpenGL",
    "PySide6.QtOpenGLWidgets", "PySide6.QtNetworkAuth", "PySide6.QtGraphs",
    "tkinter", "unittest", "pydoc_data",
    # 体积优化：项目用不到的重型依赖（psutil/qrcode 均不需要 numpy）
    "numpy", "numpy.libs", "numpy._core", "numpy.random", "numpy.fft", "numpy.linalg",
]

# 二进制层排除：软件 OpenGL 备胎、Qt Qml/Quick/Pdf/Network 运行库、PIL 冷门编解码
BIN_EXCLUDES = {
    "opengl32sw.dll",
    "qt6quick.dll", "qt6qml.dll", "qt6qmlmodels.dll", "qt6qmlmeta.dll",
    "qt6qmlworkerscript.dll", "qt6pdf.dll", "qt6quickwidgets.dll", "qt6network.dll",
    "_avif.cp310-win_amd64.pyd",
}


def _keep(binaries):
    """按目标文件名排除 BIN_EXCLUDES（不区分大小写）。"""
    return [(dest, src, kind) for dest, src, kind in binaries
            if dest.rsplit("\\", 1)[-1].rsplit("/", 1)[-1].lower() not in BIN_EXCLUDES]


a = Analysis(
    ["launcher.py"],
    pathex=[],
    binaries=[],
    datas=[],
    hiddenimports=[],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=QT_HEAVY,
    noarchive=False,
)

a.binaries = _keep(a.binaries)

pyz = PYZ(a.pure, a.zipped_data)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name="SysKit",
    debug=False,
    strip=False,
    upx=False,
    console=False,
    icon="assets/app.ico",
    uac_admin=True,
)
