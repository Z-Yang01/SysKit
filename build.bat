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
REM 只实测 dist\SysKit.exe 自身可写性：Windows 仅锁定「被运行的那个文件」，
REM 安装版实例（exe/数据在其它目录）不锁 dist，不应阻断打包；独占打开一瞬即关，不改内容
if exist dist\SysKit.exe (
  powershell -NoProfile -Command "try{$s=[IO.File]::Open('dist\SysKit.exe','Open','ReadWrite','None');$s.Close()}catch{exit 1}"
  if errorlevel 1 (
    echo dist\SysKit.exe 被占用——应是从本目录 dist 启动的 SysKit 尚未退出。
    echo 请退出后重试；若运行的是安装版实例，无需退出可直接打包。
    goto :err
  )
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
