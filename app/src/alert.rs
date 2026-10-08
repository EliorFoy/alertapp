//! 提醒动作定义与分发：亮屏 / 震动 / 响铃 / 弹窗，可任意组合。

use serde::{Deserialize, Serialize};

/// MQTT 消息负载。非 JSON 的纯文本消息会按「默认动作的文本提醒」处理。
///
/// ```json
/// {"title":"服务器告警","body":"CPU 95%","sender":"alertapp-abc123",
///  "actions":["popup","screen","vibrate","ring"]}
/// ```
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct AlertPayload {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub sender: String,
    #[serde(default)]
    pub actions: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Actions {
    pub popup: bool,
    pub screen: bool,
    pub vibrate: bool,
    pub ring: bool,
}

impl Actions {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn from_config(c: &crate::config::ActionsConfig) -> Self {
        Self { popup: c.popup, screen: c.screen, vibrate: c.vibrate, ring: c.ring }
    }

    /// 解析发送方请求的动作名（中英文别名都收）
    pub fn parse(list: &[String]) -> Self {
        let mut a = Self::none();
        for s in list {
            match s.trim().to_ascii_lowercase().as_str() {
                "popup" | "notify" | "notification" | "弹窗" | "通知" => a.popup = true,
                "screen" | "wake" | "screen_on" | "亮屏" => a.screen = true,
                "vibrate" | "shake" | "震动" => a.vibrate = true,
                "ring" | "sound" | "alarm" | "响铃" | "铃声" => a.ring = true,
                _ => {}
            }
        }
        a
    }

    pub fn is_empty(&self) -> bool {
        !(self.popup || self.screen || self.vibrate || self.ring)
    }

    /// 发送方请求 ∩ 本机开关
    pub fn intersect(&self, limit: &Self) -> Self {
        Self {
            popup: self.popup && limit.popup,
            screen: self.screen && limit.screen,
            vibrate: self.vibrate && limit.vibrate,
            ring: self.ring && limit.ring,
        }
    }

    pub fn label(&self) -> String {
        let mut v = Vec::new();
        if self.popup { v.push("弹窗"); }
        if self.screen { v.push("亮屏"); }
        if self.vibrate { v.push("震动"); }
        if self.ring { v.push("响铃"); }
        if v.is_empty() { "仅记录".to_string() } else { v.join("＋") }
    }
}

/// 按动作依次触发平台提醒（顺序：先亮屏，再震动/响铃，最后弹窗置顶）。
pub fn fire(a: Actions, title: &str, body: &str) {
    if a.is_empty() {
        return;
    }
    if a.screen {
        crate::platform::wake_screen();
    }
    if a.vibrate {
        crate::platform::vibrate(700);
    }
    if a.ring {
        crate::platform::ring();
    }
    if a.popup {
        crate::platform::popup(title, body);
    }
}
