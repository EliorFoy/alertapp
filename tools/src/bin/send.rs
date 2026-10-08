//! 命令行发布工具：从 PC 给订阅设备发提醒。
//!
//! 用法：
//!   alertapp-send --broker broker.emqx.io --topic alertapp/demo/alerts ^
//!     --title "服务器告警" --body "CPU 95%" --actions popup,screen,vibrate,ring [--tls] [--user u --pass p]

use alertapp::alert::AlertPayload;
use alertapp::config::Config;
use alertapp::mqtt::build_options;
use rumqttc::{Event, Outgoing, QoS};

fn main() {
    let mut cfg = Config::default();
    let mut title = "提醒".to_string();
    let mut body = String::new();
    let mut actions: Vec<String> = vec!["popup".into(), "vibrate".into(), "ring".into()];

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut val = || args.next().unwrap_or_default();
        match arg.as_str() {
            "--broker" => cfg.broker = val(),
            "--port" => cfg.port = val().parse().unwrap_or(cfg.port),
            "--topic" => cfg.topic = val(),
            "--user" => cfg.username = val(),
            "--pass" => cfg.password = val(),
            "--ca" => cfg.ca_path = Some(val()),
            "--tls" => cfg.use_tls = true,
            "--title" => title = val(),
            "--body" => body = val(),
            "--actions" => {
                actions = val()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "--help" | "-h" => {
                println!("参数：--broker --port --topic --user --pass --ca --tls --title --body --actions popup,screen,vibrate,ring");
                return;
            }
            other => eprintln!("忽略未知参数: {other}"),
        }
    }

    let payload = AlertPayload { title, body, sender: "cli".into(), actions };
    let bytes = match serde_json::to_vec(&payload) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("✘ 序列化失败: {e}");
            std::process::exit(1);
        }
    };
    let topic = cfg.topic.clone();

    let (client, mut connection) =
        rumqttc::Client::new(build_options(&cfg).expect("MQTT 配置错误"), 4);
    if let Err(e) = client.publish(&topic, QoS::AtLeastOnce, false, bytes) {
        eprintln!("✘ 发布失败: {e}");
        std::process::exit(1);
    }
    // 等待 Broker 确认（QoS1 PUBACK）；最多等 40 秒，避免网络异常时无限挂起
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
    loop {
        if std::time::Instant::now() > deadline {
            println!("⚠ 已提交但 40 秒内未收到 Broker 确认，消息可能已送达");
            std::process::exit(2);
        }
        match connection.recv_timeout(std::time::Duration::from_millis(500)) {
            // PUBLISH 写入网络即认为已提交（QoS1 下 Broker 掉线重连会补投）
            Ok(Ok(Event::Outgoing(Outgoing::Publish(_)))) | Ok(Ok(Event::Outgoing(Outgoing::PubAck(_)))) => {
                println!("✔ 已发送 {topic}");
                return;
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                eprintln!("✘ 连接失败: {e}");
                std::process::exit(1);
            }
            Err(_) => {} // 等待超时，继续
        }
    }
}
