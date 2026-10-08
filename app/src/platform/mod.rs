//! 平台抽象：Android 用 JNI 调系统能力，桌面端提供最小等价实现。

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub use android as imp;

#[cfg(not(target_os = "android"))]
pub mod desktop;
#[cfg(not(target_os = "android"))]
pub use desktop as imp;

/// 弹窗提醒（Android：高优先级通知 + 全屏意图，息屏时会点亮屏幕）
pub fn popup(title: &str, body: &str) {
    imp::popup(title, body)
}

/// 点亮屏幕（短暂唤醒，约 15 秒后自动熄灭）
pub fn wake_screen() {
    imp::wake_screen()
}

/// 震动一次，时长毫秒
pub fn vibrate(ms: u64) {
    imp::vibrate(ms)
}

/// 设置提醒铃声（kind：0=系统通知音 1=系统闹钟音 2=系统来电铃）与响铃时长
/// （mode：0=响一声 1=3秒 2=8秒 3=15秒 4=30秒）
pub fn set_ring_config(kind: u32, mode: u32) {
    imp::set_ring_config(kind, mode)
}

/// 播放一次指定的系统提示音（试听，最多 6 秒）
pub fn preview_ringtone(kind: u32) {
    imp::preview_ringtone(kind)
}

/// 按当前铃声配置响铃
pub fn ring() {
    imp::ring()
}

/// 后台保活开关（Android：长期持有 PARTIAL_WAKE_LOCK，息屏时保持连接）
pub fn apply_keepalive(on: bool) {
    imp::apply_keepalive(on)
}

pub fn open_battery_settings() {
    imp::open_battery_settings()
}

/// 尽力查询是否已在电池优化白名单（None = 当前 ROM 无法检测）
pub fn is_battery_ignored() -> Option<bool> {
    imp::is_battery_ignored()
}

pub fn open_notif_settings() {
    imp::open_notif_settings()
}

/// 配置文件目录
pub fn config_dir() -> std::path::PathBuf {
    imp::config_dir()
}
