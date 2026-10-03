@echo off
chcp 65001 >nul
:: ============================================================
:: SysKit Rust 版真机冒烟一键启动器（自提权）
:: 双击运行 → UAC 点「是」→ 全自动跑 12 项冒烟 → 结果写 smoke_result.txt
:: 期间会自动开关窗口并注入按键，【请勿触碰键鼠】约 5-10 分钟
:: ============================================================

:: —— 自提权：非管理员则以管理员重启自身 ——
net session >nul 2>&1
if %errorlevel% neq 0 (
    echo 请求管理员权限（UAC 弹窗请点「是」）...
    powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
    exit /b
)

cd /d E:\Code\project\SysKit

:: —— 前置 1：Rust 版应用必须在运行（没有就启动）——
tasklist /FI "IMAGENAME eq syskit-rs.exe" 2>nul | find /I "syskit-rs.exe" >nul
if %errorlevel% neq 0 (
    echo [前置] 启动 SysKit Rust 版...
    start "" "E:\Code\project\SysKit\app-rs\src-tauri\target\release\syskit-rs.exe"
    timeout /t 6 /nobreak >nul
)
tasklist /FI "IMAGENAME eq syskit-rs.exe" 2>nul | find /I "syskit-rs.exe" >nul
if %errorlevel% neq 0 (
    echo [错误] SysKit 未运行且启动失败——中止。请先手动启动应用再跑本脚本。
    pause
    exit /b 1
)

:: —— 前置 2：监听必须开启（settings.json 已为 true 则跳过）——
echo [前置] 监听状态已由 settings.json 承载（clipboard_watch=true）。

echo.
echo ============================================================
echo  冒烟开始：会自动开关窗口并注入按键
echo  【请勿触碰键鼠】约 5-10 分钟；中途可能再弹一次 UAC（点「是」）
echo ============================================================
echo.

python -m pytest tools\test_smoke_real_rs.py --run-real -v > "%~dp0..\smoke_result.txt" 2>&1

echo.
echo ============================================================
echo  冒烟结束。结果摘要（全文见仓库根 smoke_result.txt）：
echo ============================================================
findstr /C:"PASSED" /C:"FAILED" /C:"SKIPPED" /C:"passed" /C:"failed" "%~dp0..\smoke_result.txt"
echo.
echo 把仓库根的 smoke_result.txt 交给助手分析即可。
pause
