@echo off
REM ============================================================
REM  SysKit 用户目录安装 / 升级（无需拷贝到 Program Files）
REM  安装位置: %LOCALAPPDATA%\Programs\SysKit
REM  可创建桌面 + 开始菜单快捷方式；覆盖旧 exe 即完成升级。
REM ============================================================
setlocal enabledelayedexpansion
cd /d %~dp0

set "DEST=%LOCALAPPDATA%\Programs\SysKit"
set "SRC=dist\SysKit.exe"

echo [1/4] 检查构建产物...
if not exist "%SRC%" (
  echo 未找到 %SRC%
  echo 请先运行 build.bat 完成打包。
  exit /b 1
)

echo [2/4] 安装到 %DEST%
if not exist "%DEST%" mkdir "%DEST%"
copy /Y "%SRC%" "%DEST%\SysKit.exe" >nul
if errorlevel 1 (
  echo 复制失败，目标可能被占用。请先退出正在运行的 SysKit 再试。
  exit /b 1
)

echo [3/4] 生成快捷方式...
powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$dest = '%DEST%'; $exe = Join-Path $dest 'SysKit.exe';" ^
  "$ws = New-Object -ComObject WScript.Shell;" ^
  "$desk = [Environment]::GetFolderPath('Desktop');" ^
  "$sm = [Environment]::GetFolderPath('Programs');" ^
  "$s1 = $ws.CreateShortcut((Join-Path $desk 'SysKit.lnk'));" ^
  "$s1.TargetPath = $exe; $s1.WorkingDirectory = $dest; $s1.Description = 'SysKit System Toolbox'; $s1.Save();" ^
  "$menu = Join-Path $sm 'SysKit'; if (-not (Test-Path $menu)) { New-Item -ItemType Directory -Path $menu | Out-Null };" ^
  "$s2 = $ws.CreateShortcut((Join-Path $menu 'SysKit.lnk'));" ^
  "$s2.TargetPath = $exe; $s2.WorkingDirectory = $dest; $s2.Save();" ^
  "Write-Host 'shortcuts ok'"

echo [4/4] 完成
echo.
echo SysKit 已安装到:
echo   %DEST%\SysKit.exe
echo 升级：重新 build.bat 后再次运行本脚本，覆盖同路径即可。
echo 卸载：删除 %DEST% 目录及桌面/开始菜单快捷方式。
echo.
choice /C YN /M "是否立即启动 SysKit"
if !errorlevel!==1 start "" "%DEST%\SysKit.exe"
exit /b 0
