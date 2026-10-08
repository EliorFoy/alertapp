//! MQTT 引擎：独立线程 + tokio current-thread runtime。
//! - clean_session=false + QoS1：掉线期间 Broker 为我们缓存消息，重连后自动补发
//! - rumqttc 事件循环自带断线自动重连（指数退避），重连成功后重新订阅
//! - 设置页保存后通过 Command::Reload 重建连接

use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, TlsConfiguration, Transport};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::Ui;

pub enum Command {
    Publish { topic: String, payload: Vec<u8> },
    Reload(Box<Config>),
}

/// 构造 MqttOptions（GUI 与命令行发送工具共用）
pub fn build_options(cfg: &Config) -> anyhow::Result<MqttOptions> {
    let id = if cfg.client_id.is_empty() {
        format!("alertapp-{:06x}", std::process::id())
    } else {
        cfg.client_id.clone()
    };
    let mut opts = MqttOptions::new(id, cfg.broker.as_str(), cfg.port);
    opts.set_keep_alive(std::time::Duration::from_secs(30));
    // 持久会话：离线消息由 Broker 补发
    opts.set_clean_session(false);
    if !cfg.username.is_empty() {
        opts.set_credentials(cfg.username.clone(), cfg.password.clone());
    }
    if cfg.use_tls {
        let tls = TlsConfiguration::Rustls(tls_client_config(cfg)?);
        opts.set_transport(Transport::Tls(tls));
    }
    Ok(opts)
}

/// rustls 配置：默认内置公共根证书（webpki-roots），也可指定自签 CA 的 PEM 文件。
/// 显式使用 ring provider，避开 aws-lc-rs 的平台编译问题。
fn tls_client_config(cfg: &Config) -> anyhow::Result<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    match cfg.ca_path.as_deref() {
        Some(path) if !path.is_empty() => {
            let f = std::fs::File::open(path)
                .map_err(|e| anyhow::anyhow!("打开 CA 文件失败 {path}: {e}"))?;
            for cert in rustls_pemfile::certs(&mut std::io::BufReader::new(f)) {
                roots.add(cert.map_err(|e| anyhow::anyhow!("解析 CA 证书失败: {e}"))?)?;
            }
        }
        _ => {
            roots.roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
    }
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// 启动 MQTT 线程，返回控制通道
pub fn spawn(cfg: Config, ui: Ui) -> mpsc::UnboundedSender<Command> {
    let (tx, rx) = mpsc::unbounded_channel();
    let _ = std::thread::Builder::new().name("mqtt".into()).spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                log::error!("tokio runtime 启动失败: {e}");
                return;
            }
        };
        rt.block_on(event_loop(cfg, rx, ui));
    });
    tx
}

async fn event_loop(mut cfg: Config, mut rx: mpsc::UnboundedReceiver<Command>, ui: Ui) {
    'outer: loop {
        let (client, mut eventloop) = match build_options(&cfg) {
            Ok(o) => AsyncClient::new(o, 16),
            Err(e) => {
                ui.status(false, &format!("MQTT 配置错误：{e}；请到设置页修改后保存"));
                match rx.recv().await {
                    Some(Command::Reload(c)) => {
                        cfg = *c;
                        continue 'outer;
                    }
                    _ => break 'outer,
                }
            }
        };
        ui.status(false, &format!("连接中 {}:{} …", cfg.broker, cfg.port));
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    None => break 'outer,
                    Some(Command::Reload(c)) => {
                        cfg = *c;
                        continue 'outer;
                    }
                    Some(Command::Publish { topic, payload }) => {
                        if let Err(e) = client.publish(&topic, QoS::AtLeastOnce, false, payload).await {
                            log::warn!("发布失败: {e}");
                        }
                    }
                },
                ev = eventloop.poll() => match ev {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        let _ = client.subscribe(&cfg.topic, QoS::AtLeastOnce).await;
                        ui.status(true, &format!("已连接 {}:{} · 订阅 {}", cfg.broker, cfg.port, cfg.topic));
                    }
                    Ok(Event::Incoming(Packet::Publish(p))) => crate::handle_incoming(&cfg, &p, &ui),
                    Ok(_) => {}
                    Err(e) => {
                        ui.status(false, &format!("连接中断，自动重连中（{e}）"));
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }
}
