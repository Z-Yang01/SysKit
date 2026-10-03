@echo off
setlocal EnableExtensions
rem ============================================================
rem SysKit Rust deploy swap (Step 5)
rem   Swaps E:\Software\SysKit\syskit-rs.exe with the freshly
rem   bundled exe (npx tauri build product, SysKit.exe).
rem
rem Usage:
rem   deploy_swap_rs.bat             exe swap only
rem   deploy_swap_rs.bat with-data   also carry %APPDATA%\SysKit
rem                                  config data into the portable
rem                                  data dir (vault db+key copied
rem                                  as a pair; both sides backed up)
rem
rem Guards: aborts if any instance is running (two versions must
rem never run simultaneously; also the exe file would be locked).
rem ============================================================

rem canonical standalone = bundle output (npx tauri build): embedded frontend,
rem verified end-to-end by the bundler. 2026-10-04: bundle renames the artifact
rem to SysKit.exe (productName) — this script intentionally keeps the deployed
rem NAME as syskit-rs.exe so it never collides with the Python SysKit.exe.
set "SRC=E:\Code\project\SysKit\app-rs\src-tauri\target\release\SysKit.exe"
set "DSTDIR=E:\Software\SysKit"
set "DST=%DSTDIR%\syskit-rs.exe"
set "APPDATA_SK=%APPDATA%\SysKit"
set "PORTABLE_SK=%DSTDIR%\data\config"

if not exist "%SRC%" (
  echo [ABORT] source exe missing: %SRC%
  echo Build it first:  cd app-rs ^&^& npx tauri build
  exit /b 1
)

rem -- Guard 1: no running Rust instance -----------------------
tasklist /FI "IMAGENAME eq syskit-rs.exe" 2>nul | find /I "syskit-rs.exe" >nul
if not errorlevel 1 (
  echo [ABORT] syskit-rs.exe is running. Quit it first: tray right-click - Exit
  exit /b 1
)

rem -- Guard 2: no running Python instance ---------------------
tasklist /FI "IMAGENAME eq SysKit.exe" 2>nul | find /I "SysKit.exe" >nul
if not errorlevel 1 (
  echo [ABORT] SysKit.exe ^(Python packaged^) is running. Quit it first.
  exit /b 1
)
tasklist /FI "IMAGENAME eq pythonw.exe" 2>nul | find /I "pythonw.exe" >nul
if not errorlevel 1 (
  echo [ABORT] pythonw.exe is running - may be the Python SysKit. Quit it first.
  exit /b 1
)

for /f %%i in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd-HHmmss"') do set "STAMP=%%i"

rem -- Mode: with-data (carry %APPDATA% data into portable dir) --
if /I "%~1"=="with-data" call :carry_data
if errorlevel 1 exit /b 1

rem -- Exe swap with backup chain -------------------------------
if exist "%DST%" (
  copy /Y "%DST%" "%DST%.bak-%STAMP%" >nul
  if errorlevel 1 ( echo [ABORT] exe backup failed & exit /b 1 )
)
copy /Y "%SRC%" "%DST%" >nul
if errorlevel 1 ( echo [ABORT] exe copy failed & exit /b 1 )

for %%S in ("%SRC%") do set "SRCSZ=%%~zS"
for %%D in ("%DST%") do set "DSTSZ=%%~zS"
if not "%SRCSZ%"=="%DSTSZ%" (
  echo [ABORT] size mismatch after copy ^(src %SRCSZ% vs dst %DSTSZ%^)
  exit /b 1
)

echo [OK] deployed %DST% ^(%DSTSZ% bytes^)
if exist "%DST%.bak-%STAMP%" echo [OK] previous exe kept as syskit-rs.exe.bak-%STAMP%
echo Next: start "%DST%"  ^(portable data dir applies: %DSTDIR%\data^)
exit /b 0

rem ------------------------------------------------------------
:carry_data
if not exist "%APPDATA_SK%\settings.json" (
  echo [ABORT] no %APPDATA_SK%\settings.json - nothing to carry
  exit /b 1
)
rem backup portable config wholesale first
if exist "%PORTABLE_SK%" (
  robocopy "%PORTABLE_SK%" "%DSTDIR%\data\config.bak-%STAMP%" /E /NFL /NDL /NJH /NJS >nul
  if errorlevel 8 ( echo [ABORT] portable config backup failed & exit /b 1 )
  echo [OK] portable config backed up to data\config.bak-%STAMP%
)
rem vault must move as a pair (db + key), key is the decryption root
if not exist "%APPDATA_SK%\vault.key" (
  echo [ABORT] %%APPDATA%%\SysKit\vault.key missing - refusing partial vault copy
  exit /b 1
)
copy /Y "%APPDATA_SK%\vault.db"  "%PORTABLE_SK%\vault.db"  >nul || ( echo [ABORT] vault.db copy failed & exit /b 1 )
copy /Y "%APPDATA_SK%\vault.key" "%PORTABLE_SK%\vault.key" >nul || ( echo [ABORT] vault.key copy failed & exit /b 1 )
for %%F in (settings.json clipboard.json events.json traffic.json netstats_buckets.json) do (
  if exist "%APPDATA_SK%\%%F" copy /Y "%APPDATA_SK%\%%F" "%PORTABLE_SK%\%%F" >nul
)
if exist "%APPDATA_SK%\clipboard_files" (
  robocopy "%APPDATA_SK%\clipboard_files" "%PORTABLE_SK%\clipboard_files" /E /NFL /NDL /NJH /NJS >nul
  if errorlevel 8 ( echo [WARN] clipboard_files partial copy & exit /b 0 )
)
echo [OK] data carried: settings/clipboard/events/traffic/netstats/vault^(db+key^)/clipboard_files
echo      source: %%APPDATA%%\SysKit  -^>  %PORTABLE_SK%
exit /b 0
