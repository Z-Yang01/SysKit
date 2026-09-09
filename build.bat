@echo off
cd /d %~dp0
echo [1/3] generate icon...
python tools\make_icon.py || goto :err
echo [2/3] install deps...
pip install -r requirements.txt -q || goto :err
echo [3/3] pyinstaller...
pyinstaller SysKit.spec --noconfirm || goto :err
echo.
echo Build OK: dist\NetTool.exe
exit /b 0
:err
echo Build FAILED
exit /b 1
