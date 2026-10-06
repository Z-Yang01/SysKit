@echo off
setlocal EnableExtensions
rem ============================================================
rem SysKit deploy swap (Rust version, the only maintained one)
rem   Swaps E:\Software\SysKit\SysKit.exe with the freshly
rem   bundled exe (npx tauri build product), keeps the previous
rem   exe in a .bak chain (2 newest kept).
rem
rem Usage:
rem   deploy_swap_rs.bat             exe swap only
rem   deploy_swap_rs.bat with-data   also carry %%APPDATA%%\SysKit
rem                                  config data into the portable
rem                                  data dir (vault db+key copied
rem                                  as a pair; both sides backed up)
rem
rem Guards: aborts if any instance is running (exe file would be
rem locked; single-instance mutex also forbids a second copy).
rem
rem NOTE: keep this file ASCII-only + CRLF. cmd parses batch in
rem the OEM codepage (GBK here); UTF-8 Chinese comments break it.
rem ============================================================

rem Deployed name unified to SysKit.exe (productName). The Python
rem version is retired (tag python-final), so the old collision-
rem avoiding name syskit-rs.exe is auto-retired to .bak on swap.
set "SRC=E:\Code\project\SysKit\app-rs\src-tauri\target\release\SysKit.exe"
set "DSTDIR=E:\Software\SysKit"
set "DST=%DSTDIR%\SysKit.exe"
set "LEGACY=%DSTDIR%\syskit-rs.exe"
set "APPDATA_SK=%APPDATA%\SysKit"
set "PORTABLE_SK=%DSTDIR%\data\config"

if not exist "%SRC%" (
  echo [ABORT] source exe missing: %SRC%
  echo Build it first:  cd app-rs ^&^& npx tauri build
  exit /b 1
)

rem -- Guard: no running instance (either historical name) ------
tasklist /FI "IMAGENAME eq SysKit.exe" 2>nul | find /I "SysKit.exe" >nul
if not errorlevel 1 (
  echo [ABORT] SysKit.exe is running. Quit it first: tray right-click - Exit
  exit /b 1
)
tasklist /FI "IMAGENAME eq syskit-rs.exe" 2>nul | find /I "syskit-rs.exe" >nul
if not errorlevel 1 (
  echo [ABORT] syskit-rs.exe ^(legacy deployed name^) is running. Quit it first.
  exit /b 1
)

for /f %%i in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd-HHmmss"') do set "STAMP=%%i"

rem -- Mode: with-data (carry APPDATA data into portable dir) --
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

rem -- Retire legacy deployed name (avoid two-launch confusion) --
if exist "%LEGACY%" (
  move /Y "%LEGACY%" "%LEGACY%.bak-%STAMP%" >nul
  echo [OK] legacy syskit-rs.exe retired as syskit-rs.exe.bak-%STAMP%
)

rem -- Prune .bak chain: keep the 2 newest ----------------------
for /f "skip=2 delims=" %%B in ('dir /b /o-n "%DSTDIR%\SysKit.exe.bak-*" 2^>nul') do (
  del /q "%DSTDIR%\%%B"
)

echo [OK] deployed %DST% ^(%DSTSZ% bytes^)
if exist "%DST%.bak-%STAMP%" echo [OK] previous exe kept as SysKit.exe.bak-%STAMP%
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
if not exist "%PORTABLE_SK%" mkdir "%PORTABLE_SK%"
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
