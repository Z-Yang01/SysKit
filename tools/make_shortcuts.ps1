# 创建 SysKit 桌面 + 开始菜单快捷方式（由 install.bat 调用）。
$dest = Join-Path $env:LOCALAPPDATA 'Programs\SysKit'
$exe = Join-Path $dest 'SysKit.exe'
if (-not (Test-Path $exe)) {
    Write-Host "not found: $exe"
    exit 1
}
$ws = New-Object -ComObject WScript.Shell

$desk = [Environment]::GetFolderPath('Desktop')
$s1 = $ws.CreateShortcut((Join-Path $desk 'SysKit.lnk'))
$s1.TargetPath = $exe
$s1.WorkingDirectory = $dest
$s1.Description = 'SysKit System Toolbox'
$s1.Save()

$menu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\SysKit'
if (-not (Test-Path $menu)) { New-Item -ItemType Directory -Path $menu | Out-Null }
$s2 = $ws.CreateShortcut((Join-Path $menu 'SysKit.lnk'))
$s2.TargetPath = $exe
$s2.WorkingDirectory = $dest
$s2.Save()

Write-Host 'shortcuts ok'
