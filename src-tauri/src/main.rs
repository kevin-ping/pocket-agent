// 防止 Windows release 模式下弹出额外控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    pocket_agent_lib::run()
}
