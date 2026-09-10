@echo off
cd /d %~dp0
echo ============================================
echo  SysKit build  (icon - deps - pyinstaller)
echo ============================================

echo.
echo [1/4] generate icon...
python tools\make_icon.py || goto :err

echo [2/4] install deps...
python -m pip install -r requirements.txt -q || goto :err

echo [3/4] pyinstaller...
pyinstaller SysKit.spec --noconfirm --clean || goto :err

echo [4/4] package portable zip...
if not exist dist mkdir dist
powershell -NoProfile -Command ^
  "$v = Select-String -Path 'syskit\config.py' -Pattern 'VERSION\s*=\s*\"([^\"]+)\"' | ForEach-Object { $_.Matches[0].Groups[1].Value };" ^
  "if (-not $v) { $v = '0.0.0' };" ^
  "$zip = Join-Path 'dist' ('SysKit-' + $v + '-portable.zip');" ^
  "if (Test-Path $zip) { Remove-Item $zip -Force };" ^
  "Compress-Archive -Path 'dist\SysKit.exe' -DestinationPath $zip -Force;" ^
  "Write-Host ('zip -> ' + $zip)"

echo.
echo Build OK
echo   exe : dist\SysKit.exe
echo   下一步: 运行 install.bat  安装/升级到用户目录并创建快捷方式
exit /b 0
:err
echo Build FAILED
exit /b 1
