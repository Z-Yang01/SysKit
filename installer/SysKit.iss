; ============================================================
;  SysKit 安装向导脚本（Inno Setup 6）
;  由 build.bat 第 5 步调用：
;    ISCC.exe /DAPP_VERSION=1.2.0 installer\SysKit.iss
;  产出: dist\SysKit-<版本>-setup.exe（下一步式安装向导）
; ============================================================
#ifndef APP_VERSION
#define APP_VERSION "0.0.0"
#endif

[Setup]
; 固定 GUID：升级版安装包自动识别并覆盖旧安装
AppId={{7E1A2C64-9B3D-4B6E-9F41-3A5C8D2E1F60}
AppName=SysKit 系统工具箱
AppVersion={#APP_VERSION}
AppPublisher=SysKit
; 程序清单要求管理员（修复动作/开机自启计划任务），安装器同权限装到 Program Files
; 允许命令行 /CURRENTUSER 覆盖：免管理员装到用户目录（%LOCALAPPDATA%\Programs\SysKit）
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=commandline
DefaultDirName={autopf}\SysKit
DisableDirPage=no
DefaultGroupName=SysKit
OutputDir=..\dist
OutputBaseFilename=SysKit-{#APP_VERSION}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\SysKit.exe
; 升级覆盖安装；卸载不动 %APPDATA%\SysKit / %LOCALAPPDATA%\SysKit 用户数据（设置/密码库/剪贴板历史）

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式(&D)"; GroupDescription: "附加任务："
Name: "portabledata"; Description: "数据保存在安装目录（便携模式：设置/密码库/日志随目录整体搬迁）(&P)"; GroupDescription: "附加任务："
Name: "autostart"; Description: "开机自动启动（后台托盘守护）(&S)"; GroupDescription: "附加任务："; Flags: unchecked

[Files]
Source: "..\dist\SysKit.exe"; DestDir: "{app}"; Flags: ignoreversion

[Dirs]
; 便携模式标记：程序检测到 exe 旁的 data 目录即把全部数据存在安装目录
Name: "{app}\data"; Tasks: portabledata

[Icons]
Name: "{group}\SysKit 系统工具箱"; Filename: "{app}\SysKit.exe"
Name: "{group}\卸载 SysKit"; Filename: "{uninstallexe}"
Name: "{autodesktop}\SysKit"; Filename: "{app}\SysKit.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\SysKit.exe"; Description: "立即启动 SysKit(&R)"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; 卸载时清理开机自启计划任务（与 syskit/autostart.py 的任务名一致）
Filename: "schtasks"; Parameters: "/Delete /TN SysKitAutostart /F"; RunOnceId: "DelAutoStart"; Flags: runhidden

[Code]
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
  Args: string;
begin
  if CurStep = ssPostInstall then
  begin
    if WizardIsTaskSelected('autostart') then
    begin
      // 与 syskit/autostart.py 相同的任务名与最高权限运行（登录静默启动，免 UAC）
      Args := '/Create /F /TN SysKitAutostart /TR "' +
              ExpandConstant('{app}') + '\SysKit.exe --minimized" /SC ONLOGON /RL HIGHEST';
      Exec('schtasks.exe', Args, '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    end;
  end;
end;
