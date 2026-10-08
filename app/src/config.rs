//! 应用配置：持久化为 JSON（Android 存在应用私有目录，桌面端在 %APPDATA%\alertapp）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 本机允许的提醒方式开关（收到消息时与发送方请求取交集）
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct ActionsConfig {
    pub popup: bool,
    pub screen: bool,
    pub vibrate: bool,
    pub ring: bool,
}

impl Default for ActionsConfig {
    fn default() -> Self {
        Self { popup: true, screen: true, vibrate: true, ring: true }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub broker: String,
    pub port: u16,
    pub topic: String,
    /// 本机设备标识：既是 MQTT client id，也用于识别“自己发的消息”
    pub client_id: String,
    pub username: String,
    pub password: String,
    pub use_tls: bool,
    /// 自定义 CA 证书（PEM）路径；为空时使用内置的公共根证书
    pub ca_path: Option<String>,
    /// 后台保活（Android：长期持有 PARTIAL_WAKE_LOCK）
    pub keep_alive: bool,
    /// 提醒铃声：0=系统通知音（短促）1=系统闹钟音 2=系统来电铃
    pub ringtone_kind: u32,
    /// 响铃时长：0=响一声 1=3秒 2=8秒 3=15秒 4=30秒
    pub ring_mode: u32,
    pub actions: ActionsConfig,
}

impl Default for Config {
    fn default() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self {
            broker: "broker.emqx.io".into(),
            port: 1883,
            topic: "alertapp/demo/alerts".into(),
            client_id: format!("alertapp-{:06x}", nanos & 0xffffff),
            username: String::new(),
            password: String::new(),
            use_tls: false,
            ca_path: None,
            keep_alive: true,
            ringtone_kind: 0,
            ring_mode: 0,
            actions: ActionsConfig::default(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    crate::platform::config_dir()
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn load() -> Config {
    match std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(c) => c,
        None => {
            let c = Config::default();
            let _ = save(&c);
            c
        }
    }
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(cfg).unwrap_or_default();
    std::fs::write(config_path(), json)
}
