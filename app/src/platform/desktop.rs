//! 桌面端（开发用）：系统通知 + 终端提示音（BEL）。亮屏/震动不可用。

use std::io::Write;

pub fn popup(title: &str, body: &str) {
    let result = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .timeout(notify_rust::Timeout::Milliseconds(10000))
        .show();
    if let Err(e) = result {
        log::warn!("桌面通知失败（不影响其他提醒方式）: {e}");
    }
    bell(3);
}

fn bell(times: usize) {
    let mut out = std::io::stdout();
    for _ in 0..times {
        let _ = out.write_all(b"\x07");
        let _ = out.flush();
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

pub fn wake_screen() {
    log::debug!("桌面端不支持亮屏");
}

pub fn vibrate(_ms: u64) {
    log::debug!("桌面端不支持震动");
}

pub fn ring() {
    // 桌面端：短促三声提示音
    bell(3);
}

pub fn preview_ringtone(_kind: u32) {
    log::info!("桌面端试听不可用");
}

pub fn set_ring_config(_kind: u32, _mode: u32) {
    // 桌面端固定短促提示音
}

pub fn apply_keepalive(_on: bool) {
    // 桌面端进程常驻，无需保活
}

pub fn open_battery_settings() {
    log::info!("桌面端无需电池白名单");
}

pub fn is_battery_ignored() -> Option<bool> {
    // 桌面端无电池优化限制
    Some(true)
}

pub fn open_notif_settings() {
    log::info!("请到系统设置中管理通知权限");
}

pub fn config_dir() -> std::path::PathBuf {
    std::env::var("APPDATA")
        .map(|d| std::path::PathBuf::from(d).join("alertapp"))
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default())
}
