// Windows GUI 子系统：release 不带控制台窗口（对应 PyInstaller console=False）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    syskit_rs_lib::run()
}
