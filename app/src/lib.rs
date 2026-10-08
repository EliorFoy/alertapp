slint::include_modules!();

pub mod alert;
pub mod config;
pub mod mqtt;
pub mod platform;

use std::cell::RefCell;
use std::rc::Rc;
use slint::{Model, ModelRc, VecModel};

// 消息列表模型只能在 UI 线程访问：用 thread_local 保存，
// 跨线程的 Ui 只持有可 Send 的 Weak<AppWindow>。
thread_local! {
    static MESSAGES: RefCell<Option<Rc<VecModel<Msg>>>> = const { RefCell::new(None) };
}

/// UI 桥：MQTT 线程通过它把消息/状态安全投递到 Slint 事件循环。
#[derive(Clone)]
pub struct Ui {
    weak: slint::Weak<AppWindow>,
}

impl Ui {
    fn status(&self, connected: bool, text: &str) {
        let weak = self.weak.clone();
        let text = text.to_string();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                let g = app.global::<AppBridge>();
                g.set_connected(connected);
                g.set_conn_status(text.into());
            }
        });
    }

    /// 追加一条消息到列表；flash=true 时顶部横幅闪烁 5 秒（收到他人提醒时）。
    fn push(&self, title: &str, body: &str, from: &str, actions: &str, flash: bool) {
        let weak = self.weak.clone();
        let (title, body, from, actions) =
            (title.to_string(), body.to_string(), from.to_string(), actions.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            let time = chrono::Local::now().format("%H:%M:%S").to_string();
            let msg = Msg {
                title: title.into(),
                body: body.into(),
                from: from.into(),
                time: time.into(),
                actions: actions.into(),
            };
            MESSAGES.with(|cell| {
                if let Some(model) = cell.borrow().as_ref() {
                    // 新消息插到顶部，无需滚动即可看到最新
                    model.insert(0, msg);
                    while model.row_count() > 200 {
                        model.remove(model.row_count() - 1);
                    }
                }
            });
            if flash {
                if let Some(app) = weak.upgrade() {
                    app.global::<AppBridge>().set_alert_flash(true);
                    let weak2 = weak.clone();
                    slint::Timer::single_shot(std::time::Duration::from_secs(5), move || {
                        if let Some(app) = weak2.upgrade() {
                            app.global::<AppBridge>().set_alert_flash(false);
                        }
                    });
                }
            }
        });
    }
}

/// 处理收到的 MQTT 消息：解析 JSON → 计算生效的提醒动作 → 更新 UI → 触发提醒。
/// 自己发的消息只进列表、不提醒（避免自问自答式打扰）。
fn handle_incoming(cfg: &config::Config, p: &rumqttc::mqttbytes::v4::Publish, ui: &Ui) {
    let text = String::from_utf8_lossy(&p.payload).to_string();
    let payload: alert::AlertPayload = serde_json::from_str(&text).unwrap_or_else(|_| alert::AlertPayload {
        title: "提醒".into(),
        body: text.clone(),
        sender: String::new(),
        actions: vec![],
    });
    let title = if payload.title.is_empty() { "提醒".to_string() } else { payload.title.clone() };
    let is_self = !payload.sender.is_empty() && payload.sender == cfg.client_id;

    let requested = alert::Actions::parse(&payload.actions);
    let effective = if is_self {
        alert::Actions::none()
    } else if requested.is_empty() {
        // 发送方没指定动作 → 用本机默认开关
        alert::Actions::from_config(&cfg.actions)
    } else {
        // 发送方指定的动作 ∩ 本机允许的开关（本机关掉的方式强制不打扰）
        requested.intersect(&alert::Actions::from_config(&cfg.actions))
    };
    let from = if payload.sender.is_empty() { "其他设备" } else { &payload.sender };
    let label = if is_self { "已发送".to_string() } else { effective.label() };

    ui.push(&title, &payload.body, from, &label, !is_self);
    if !is_self {
        log::info!("收到提醒: {title} -> {:?}", effective);
        alert::fire(effective, &title, &payload.body);
    }
}

pub fn run() -> anyhow::Result<()> {
    #[cfg(not(target_os = "android"))]
    let _ = env_logger::try_init();

    let cfg = config::load();
    let app = AppWindow::new()?;

    let model = Rc::new(VecModel::<Msg>::default());
    MESSAGES.with(|cell| *cell.borrow_mut() = Some(model.clone()));
    {
        let g = app.global::<AppBridge>();
        g.set_messages(ModelRc::from(model.clone()));
        g.set_conn_status("连接中…".into());
        g.set_broker(cfg.broker.as_str().into());
        g.set_port(cfg.port.to_string().into());
        g.set_topic(cfg.topic.as_str().into());
        g.set_username(cfg.username.as_str().into());
        g.set_password(cfg.password.as_str().into());
        g.set_use_tls(cfg.use_tls);
        g.set_keep_alive(cfg.keep_alive);
        g.set_ringtone_index(cfg.ringtone_kind as i32);
        g.set_ring_mode_index(cfg.ring_mode as i32);
        g.set_battery_ok(platform::is_battery_ignored().unwrap_or(false));
        g.set_act_popup(cfg.actions.popup);
        g.set_act_screen(cfg.actions.screen);
        g.set_act_vibrate(cfg.actions.vibrate);
        g.set_act_ring(cfg.actions.ring);
    }

    let ui = Ui { weak: app.as_weak() };
    platform::set_ring_config(cfg.ringtone_kind, cfg.ring_mode);
    platform::apply_keepalive(cfg.keep_alive);
    let tx = mqtt::spawn(cfg.clone(), ui.clone());

    // ── 发布页：发送提醒 ─────────────────────────────────────────
    {
        let weak = app.as_weak();
        let tx = tx.clone();
        let sender = cfg.client_id.clone();
        app.global::<AppBridge>().on_publish(move || {
            let Some(app) = weak.upgrade() else { return };
            let g = app.global::<AppBridge>();
            let mut actions = Vec::new();
            if g.get_pub_popup() { actions.push("popup".to_string()); }
            if g.get_pub_screen() { actions.push("screen".to_string()); }
            if g.get_pub_vibrate() { actions.push("vibrate".to_string()); }
            if g.get_pub_ring() { actions.push("ring".to_string()); }
            let payload = alert::AlertPayload {
                title: g.get_pub_title().to_string(),
                body: g.get_pub_body().to_string(),
                sender: sender.clone(),
                actions,
            };
            let topic = g.get_topic().to_string();
            match serde_json::to_vec(&payload) {
                Ok(bytes) => {
                    log::info!("发布到 {topic}");
                    let _ = tx.send(mqtt::Command::Publish { topic, payload: bytes });
                    g.set_pub_title("".into());
                    g.set_pub_body("".into());
                }
                Err(e) => log::error!("序列化失败: {e}"),
            }
        });
    }

    // ── 设置页：保存并重连 ───────────────────────────────────────
    {
        let weak = app.as_weak();
        let tx = tx.clone();
        let old = cfg.clone();
        app.global::<AppBridge>().on_save_settings(move || {
            let Some(app) = weak.upgrade() else { return };
            let g = app.global::<AppBridge>();
            let new_cfg = config::Config {
                broker: g.get_broker().to_string(),
                port: g.get_port().parse::<u16>().unwrap_or(old.port),
                topic: g.get_topic().to_string(),
                client_id: old.client_id.clone(),
                username: g.get_username().to_string(),
                password: g.get_password().to_string(),
                use_tls: g.get_use_tls(),
                ca_path: old.ca_path.clone(),
                keep_alive: g.get_keep_alive(),
                ringtone_kind: g.get_ringtone_index().max(0) as u32,
                ring_mode: g.get_ring_mode_index().max(0) as u32,
                actions: config::ActionsConfig {
                    popup: g.get_act_popup(),
                    screen: g.get_act_screen(),
                    vibrate: g.get_act_vibrate(),
                    ring: g.get_act_ring(),
                },
            };
            if let Err(e) = config::save(&new_cfg) {
                log::error!("保存配置失败: {e}");
            }
            platform::set_ring_config(new_cfg.ringtone_kind, new_cfg.ring_mode);
            platform::apply_keepalive(new_cfg.keep_alive);
            let _ = tx.send(mqtt::Command::Reload(Box::new(new_cfg)));
            g.set_connected(false);
            g.set_conn_status("正在重连…".into());
        });
    }

    // ── 测试提醒 / 系统设置入口 / 清空 ────────────────────────────
    {
        let weak = app.as_weak();
        app.global::<AppBridge>().on_test_alert(move || {
            let Some(app) = weak.upgrade() else { return };
            let g = app.global::<AppBridge>();
            let actions = alert::Actions {
                popup: g.get_act_popup(),
                screen: g.get_act_screen(),
                vibrate: g.get_act_vibrate(),
                ring: g.get_act_ring(),
            };
            alert::fire(actions, "测试提醒", "如果你看到/听到了这条提醒，说明提醒通道正常。");
            ui.push("测试提醒", "本机触发的测试提醒", "本机", &actions.label(), false);
        });
    }
    {
        let weak = app.as_weak();
        app.global::<AppBridge>().on_open_battery(move || {
            // 已在白名单 → 不再重复弹系统确认框
            if platform::is_battery_ignored() == Some(true) {
                log::info!("已在电池优化白名单中，无需重复设置");
                return;
            }
            platform::open_battery_settings();
        });
    }
    app.global::<AppBridge>().on_open_notif(platform::open_notif_settings);
    // 进入设置页 / 启动时刷新白名单状态（尽力检测，检测不到保持原样）
    {
        let weak = app.as_weak();
        app.global::<AppBridge>().on_settings_opened(move || {
            if let Some(app) = weak.upgrade() {
                if let Some(ok) = platform::is_battery_ignored() {
                    app.global::<AppBridge>().set_battery_ok(ok);
                }
            }
        });
    }
    {
        let weak = app.as_weak();
        app.global::<AppBridge>().on_preview_ringtone(move || {
            if let Some(app) = weak.upgrade() {
                let kind = app.global::<AppBridge>().get_ringtone_index().max(0) as u32;
                platform::preview_ringtone(kind);
            }
        });
    }
    {
        let model = model.clone();
        app.global::<AppBridge>().on_clear_messages(move || model.set_vec(Vec::new()));
    }

    app.run()?;
    Ok(())
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: slint::android::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("alertapp"),
    );
    slint::android::init(app).expect("slint android 初始化失败");
    if let Err(e) = run() {
        log::error!("应用异常退出: {e:?}");
    }
}
