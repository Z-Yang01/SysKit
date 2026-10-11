@echo off
rem ============================================================
rem  SysKit Rust smoke test - self-elevating one-click launcher
rem  Double-click -> UAC "Yes" -> runs 12 smoke targets
rem  Results: E:\Code\project\SysKit\smoke_result.txt
rem  DO NOT touch keyboard/mouse while running (~5-10 min)
rem ============================================================

rem ---- self-elevate if not admin ----
net session >nul 2>&1
if %errorlevel% neq 0 (
    echo Requesting admin rights - click YES on the UAC prompt...
    powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
    exit /b
)

cd /d E:\Code\project\SysKit

rem ---- pre 0: preset hotkeys/watch in settings (guards against poisoned values) ----
rem 2026-10-11: deployed exe uses the PORTABLE data dir (E:\Software\SysKit\data\config),
rem NOT %APPDATA% - preset BOTH so whichever exists takes effect.
powershell -NoProfile -Command ^
 "$p='%APPDATA%\SysKit\settings.json';" ^
 "if (Test-Path $p) {" ^
 "$s=(Get-Content $p -Raw | ConvertFrom-Json);" ^
 "$s.clip_paste_hotkey='Ctrl+Alt+B';" ^
 "$s.clip_quick_hotkey='Ctrl+Alt+V';" ^
 "$s.clipboard_watch=$true;" ^
 "$s | ConvertTo-Json -Depth 5 | Set-Content $p -Encoding UTF8 }"
powershell -NoProfile -Command ^
 "$p='E:\Software\SysKit\data\config\settings.json';" ^
 "if (Test-Path $p) {" ^
 "$s=(Get-Content $p -Raw | ConvertFrom-Json);" ^
 "$s.clip_paste_hotkey='Ctrl+Alt+B';" ^
 "$s.clip_quick_hotkey='Ctrl+Alt+V';" ^
 "$s.clipboard_watch=$true;" ^
 "$s | ConvertTo-Json -Depth 5 | Set-Content $p -Encoding UTF8 }"
echo [pre] settings preset (appdata + portable): panel=Ctrl+Alt+B quick=Ctrl+Alt+V watch=on

rem ---- pre 1: the Rust app must be running (start if not) ----
rem 2026-10-04: tauri bundle renames the artifact to SysKit.exe (productName);
rem the instance guard must cover BOTH names or a bundled instance is invisible.
tasklist /FI "IMAGENAME eq syskit-rs.exe" 2>nul | find /I "syskit-rs.exe" >nul
if %errorlevel% neq 0 tasklist /FI "IMAGENAME eq SysKit.exe" 2>nul | find /I "SysKit.exe" >nul
if %errorlevel% neq 0 (
    echo [pre] starting SysKit Rust app...
    rem 2026-10-11: DEPLOYED exe first - its portable data dir is the one the
    rem settings preset above wrote; starting a build-output exe would resolve
    rem a different data dir and ignore the preset.
    if exist "E:\Software\SysKit\SysKit.exe" (
        start "" "E:\Software\SysKit\SysKit.exe"
    ) else if exist "E:\Code\project\SysKit\app-rs\src-tauri\target\release\SysKit.exe" (
        start "" "E:\Code\project\SysKit\app-rs\src-tauri\target\release\SysKit.exe"
    ) else if exist "E:\Code\project\SysKit\app-rs\src-tauri\target-deploy\release\syskit-rs.exe" (
        start "" "E:\Code\project\SysKit\app-rs\src-tauri\target-deploy\release\syskit-rs.exe"
    ) else (
        start "" "E:\Code\project\SysKit\app-rs\src-tauri\target\release\syskit-rs.exe"
    )
    timeout /t 6 /nobreak >nul
)
tasklist /FI "IMAGENAME eq syskit-rs.exe" 2>nul | find /I "syskit-rs.exe" >nul
if %errorlevel% neq 0 tasklist /FI "IMAGENAME eq SysKit.exe" 2>nul | find /I "SysKit.exe" >nul
if %errorlevel% neq 0 (
    echo [ERROR] SysKit not running and failed to start. Abort.
    pause
    exit /b 1
)

echo.
echo ============================================================
echo  Smoke test starting: windows will open/close and keys
echo  will be injected automatically. DO NOT touch anything.
echo  ~5-10 min. One more UAC prompt may appear - click Yes.
echo ============================================================
echo.

python -m pytest tools\test_smoke_real_rs.py --run-real -v > "%~dp0..\smoke_result.txt" 2>&1

echo.
echo ============================================================
echo  Smoke finished. Summary (full text: smoke_result.txt):
echo ============================================================
findstr /C:"PASSED" /C:"FAILED" /C:"SKIPPED" /C:"passed" /C:"failed" "%~dp0..\smoke_result.txt"
echo.
pause
