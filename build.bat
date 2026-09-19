@echo off
chcp 65001 >nul
cd /d %~dp0
echo ============================================
echo  SysKit build  (icon - deps - pyinstaller)
echo ============================================

echo.
echo [1/5] generate icon...
python tools\make_icon.py || goto :err

echo [2/5] install deps...
python -m pip install -r requirements.txt -q || goto :err

echo [3/5] pyinstaller...
tasklist /FI "IMAGENAME eq SysKit.exe" 2>nul | findstr /I "SysKit.exe" >nul
if not errorlevel 1 (
  echo 检测到正在运行的 SysKit，它锁定了 dist\SysKit.exe，无法覆盖。
  echo 请先退出（托盘右键 - 退出）后重试。
  goto :err
)
REM 用 python -m 调用：pip --user 安装时 Scripts 目录不在 PATH，裸 pyinstaller 会找不到命令
python -m PyInstaller SysKit.spec --noconfirm --clean || goto :err
if not exist dist\SysKit.exe (
  echo 构建流程结束但未生成 dist\SysKit.exe，请查看上方日志
  goto :err
)

echo [4/5] package portable zip...
python tools\package_zip.py || goto :err

echo [5/5] build setup wizard (Inno Setup)...
for /f "delims=" %%v in ('python tools\package_zip.py --print-version') do set "VER=%%v"
if not defined VER goto :err
set "ISCC="
if exist "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" set "ISCC=%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
if exist "%ProgramFiles%\Inno Setup 6\ISCC.exe" set "ISCC=%ProgramFiles%\Inno Setup 6\ISCC.exe"
if exist "%ProgramFiles%\Inno Setup 7\ISCC.exe" set "ISCC=%ProgramFiles%\Inno Setup 7\ISCC.exe"
if exist "%LocalAppData%\Programs\Inno Setup 6\ISCC.exe" set "ISCC=%LocalAppData%\Programs\Inno Setup 6\ISCC.exe"
if exist "%LocalAppData%\Programs\Inno Setup 7\ISCC.exe" set "ISCC=%LocalAppData%\Programs\Inno Setup 7\ISCC.exe"
if not defined ISCC where ISCC >nul 2>nul && set "ISCC=ISCC"
if defined ISCC (
  "%ISCC%" /DAPP_VERSION=%VER% installer\SysKit.iss || goto :err
) else (
  echo   未找到 Inno Setup，跳过安装向导（仅影响 setup.exe）。
  echo   安装一次即可: https://jrsoftware.org/isdl.php
)

echo.
echo Build OK
echo   exe : dist\SysKit.exe
echo   zip : dist\SysKit-%VER%-portable.zip
if exist dist\SysKit-%VER%-setup.exe echo   setup: dist\SysKit-%VER%-setup.exe  （下一步式安装向导）
exit /b 0
:err
echo Build FAILED
exit /b 1
