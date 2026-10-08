//! Android 端：通过 JNI 调用系统能力实现提醒与保活。
//!
//! - 弹窗：高优先级通知 + 全屏意图（息屏/锁屏时点亮屏幕并弹出）
//! - 亮屏：SCREEN_BRIGHT + ACQUIRE_CAUSES_WAKEUP 短时唤醒锁（15 秒）
//! - 震动：VibrationEffect.createOneShot
//! - 响铃：MediaPlayer 循环播放系统闹钟铃声 N 秒
//! - 保活：长期持有 PARTIAL_WAKE_LOCK（息屏时保持 CPU/网络活动）
//!
//! 说明：cargo-apk 打包无法内置 Java/Kotlin 代码，因此没有前台服务；
//! 保活靠「唤醒锁 + 电池优化白名单 + 厂商后台白名单」组合，详见 README。

use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

const CHANNEL_ID: &str = "alertapp_alerts";
static NOTIF_ID: AtomicI32 = AtomicI32::new(1);
static RING_KIND: AtomicU32 = AtomicU32::new(0);
static RING_MODE: AtomicU32 = AtomicU32::new(0);
static KEEPALIVE_TX: OnceLock<std::sync::mpsc::Sender<bool>> = OnceLock::new();

/// 拿到已 attach 的 JNIEnv + 应用 Context 执行一段 JNI 逻辑。
/// 出错时打日志并清掉挂起的 Java 异常，绝不 panic。
fn with_jvm<T>(f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<T>) -> Option<T> {
    let actx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(actx.vm().cast::<jni::sys::JavaVM>()) }.ok()?;
    let context = unsafe { JObject::from_raw(actx.context().cast::<jni::sys::_jobject>()) };
    let mut guard = vm.attach_current_thread().ok()?;
    let env = &mut guard;
    match f(env, &context) {
        Ok(v) => Some(v),
        Err(e) => {
            log::error!("JNI 调用失败: {e}");
            // 打印挂起的 Java 异常堆栈到 logcat（System.err），便于定位
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            None
        }
    }
}

fn to_rust(env: &mut JNIEnv, j: &JString) -> jni::errors::Result<String> {
    Ok(env.get_string(j)?.into())
}

fn package_name(env: &mut JNIEnv, ctx: &JObject) -> jni::errors::Result<String> {
    let s = env.call_method(ctx, "getPackageName", "()Ljava/lang/String;", &[])?.l()?;
    to_rust(env, &JString::from(s))
}

fn get_service<'local>(
    env: &mut JNIEnv<'local>,
    ctx: &JObject,
    name: &str,
) -> jni::errors::Result<JObject<'local>> {
    let s = env.new_string(name)?;
    env.call_method(ctx, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[
        JValue::Object(&s),
    ])?
    .l()
}

fn sdk_int(env: &mut JNIEnv) -> jni::errors::Result<i32> {
    env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()
}

/// 创建（若不存在）静音的高优先级通知渠道：
/// 声音/震动由「提醒动作」显式触发，避免与响铃叠加。
fn ensure_channel(env: &mut JNIEnv, ctx: &JObject) -> jni::errors::Result<()> {
    if sdk_int(env)? < 26 {
        return Ok(());
    }
    let nm = get_service(env, ctx, "notification")?;
    if nm.is_null() {
        return Ok(());
    }
    let id = env.new_string(CHANNEL_ID)?;
    let existing = env.call_method(&nm, "getNotificationChannel", "(Ljava/lang/String;)Landroid/app/NotificationChannel;", &[
        JValue::Object(&id),
    ])?
    .l()?;
    if !existing.is_null() {
        return Ok(());
    }
    let name = env.new_string("即时提醒")?;
    let importance = env.get_static_field("android/app/NotificationManager", "IMPORTANCE_HIGH", "I")?.i()?;
    let channel = env.new_object("android/app/NotificationChannel", "(Ljava/lang/String;Ljava/lang/CharSequence;I)V", &[
        JValue::Object(&id),
        JValue::Object(&name),
        JValue::Int(importance),
    ])?;
    let null_obj = JObject::null();
    env.call_method(&channel, "setSound", "(Landroid/net/Uri;Landroid/media/AudioAttributes;)V", &[
        JValue::Object(&null_obj),
        JValue::Object(&null_obj),
    ])?;
    env.call_method(&channel, "enableVibration", "(Z)V", &[JValue::Bool(0)])?;
    env.call_method(&nm, "createNotificationChannel", "(Landroid/app/NotificationChannel;)V", &[
        JValue::Object(&channel),
    ])?;
    Ok(())
}

/// 打开本应用主界面的 Intent（通过 MAIN/LAUNCHER 解析，不依赖 Java 类名）
fn launcher_intent<'local>(
    env: &mut JNIEnv<'local>,
    pkg: &str,
) -> jni::errors::Result<JObject<'local>> {
    let action = env.new_string("android.intent.action.MAIN")?;
    let category = env.new_string("android.intent.category.LAUNCHER")?;
    let jpkg = env.new_string(pkg)?;
    let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[
        JValue::Object(&action),
    ])?;
    env.call_method(&intent, "addCategory", "(Ljava/lang/String;)Landroid/content/Intent;", &[
        JValue::Object(&category),
    ])?;
    env.call_method(&intent, "setPackage", "(Ljava/lang/String;)Landroid/content/Intent;", &[
        JValue::Object(&jpkg),
    ])?;
    let new_task = env.get_static_field("android/content/Intent", "FLAG_ACTIVITY_NEW_TASK", "I")?.i()?;
    let single_top = env.get_static_field("android/content/Intent", "FLAG_ACTIVITY_SINGLE_TOP", "I")?.i()?;
    env.call_method(&intent, "setFlags", "(I)Landroid/content/Intent;", &[JValue::Int(new_task | single_top)])?;
    Ok(intent)
}

pub fn popup(title: &str, body: &str) {
    with_jvm(|env, ctx| {
        ensure_channel(env, ctx)?;
        let pkg = package_name(env, ctx)?;
        let jpkg = env.new_string(&pkg)?;

        // 通知小图标：优先 res/drawable/ic_stat_alert，回退到应用图标
        let res = env.call_method(ctx, "getResources", "()Landroid/content/res/Resources;", &[])?.l()?;
        let icon_name = env.new_string("ic_stat_alert")?;
        let drawable = env.new_string("drawable")?;
        let mut icon_id = env.call_method(&res, "getIdentifier", "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I", &[
            JValue::Object(&icon_name),
            JValue::Object(&drawable),
            JValue::Object(&jpkg),
        ])?
        .i()?;
        if icon_id == 0 {
            let ai = env.call_method(ctx, "getApplicationInfo", "()Landroid/content/pm/ApplicationInfo;", &[])?.l()?;
            icon_id = env.get_field(&ai, "icon", "I")?.i()?;
        }

        let jtitle = env.new_string(title)?;
        let jbody = env.new_string(body)?;
        let channel = env.new_string(CHANNEL_ID)?;
        let builder = env.new_object("android/app/Notification$Builder", "(Landroid/content/Context;Ljava/lang/String;)V", &[
            JValue::Object(ctx),
            JValue::Object(&channel),
        ])?;
        env.call_method(&builder, "setSmallIcon", "(I)Landroid/app/Notification$Builder;", &[JValue::Int(icon_id)])?;
        env.call_method(&builder, "setContentTitle", "(Ljava/lang/CharSequence;)Landroid/app/Notification$Builder;", &[
            JValue::Object(&jtitle),
        ])?;
        env.call_method(&builder, "setContentText", "(Ljava/lang/CharSequence;)Landroid/app/Notification$Builder;", &[
            JValue::Object(&jbody),
        ])?;
        env.call_method(&builder, "setAutoCancel", "(Z)Landroid/app/Notification$Builder;", &[JValue::Bool(1)])?;
        let category = env.new_string("alarm")?;
        env.call_method(&builder, "setCategory", "(Ljava/lang/String;)Landroid/app/Notification$Builder;", &[
            JValue::Object(&category),
        ])?;
        let prio = env.get_static_field("android/app/Notification", "PRIORITY_MAX", "I")?.i()?;
        env.call_method(&builder, "setPriority", "(I)Landroid/app/Notification$Builder;", &[JValue::Int(prio)])?;

        let intent = launcher_intent(env, &pkg)?;
        let pi_flags = env.get_static_field("android/app/PendingIntent", "FLAG_IMMUTABLE", "I")?.i()?
            | env.get_static_field("android/app/PendingIntent", "FLAG_UPDATE_CURRENT", "I")?.i()?;
        let pi = env.call_static_method("android/app/PendingIntent", "getActivity", "(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;", &[
            JValue::Object(ctx),
            JValue::Int(0),
            JValue::Object(&intent),
            JValue::Int(pi_flags),
        ])?
        .l()?;
        env.call_method(&builder, "setContentIntent", "(Landroid/app/PendingIntent;)Landroid/app/Notification$Builder;", &[
            JValue::Object(&pi),
        ])?;
        // 全屏意图：息屏/锁屏时直接点亮屏幕弹出
        env.call_method(&builder, "setFullScreenIntent", "(Landroid/app/PendingIntent;Z)Landroid/app/Notification$Builder;", &[
            JValue::Object(&pi),
            JValue::Bool(1),
        ])?;

        let notif = env.call_method(&builder, "build", "()Landroid/app/Notification;", &[])?.l()?;
        let nm = get_service(env, ctx, "notification")?;
        let id = NOTIF_ID.fetch_add(1, Ordering::Relaxed);
        env.call_method(&nm, "notify", "(ILandroid/app/Notification;)V", &[
            JValue::Int(id),
            JValue::Object(&notif),
        ])?;
        Ok(())
    });
}

pub fn vibrate(ms: u64) {
    with_jvm(|env, ctx| {
        let sdk = sdk_int(env)?;
        let vib = if sdk >= 31 {
            let vm = get_service(env, ctx, "vibrator_manager")?;
            env.call_method(&vm, "getDefaultVibrator", "()Landroid/os/Vibrator;", &[])?.l()?
        } else {
            get_service(env, ctx, "vibrator")?
        };
        if vib.is_null() {
            return Ok(());
        }
        let half = (ms / 2).max(100) as i64;

        // 部分 ROM（云机/裁剪框架）缺失 VibrationEffect/属性类，逐级尝试并清异常降级。
        // 注意：body 里的 `?` 由立即调用的内层闭包就地捕获，不会穿透到外层。
        macro_rules! try_vibrate {
            ($body:expr) => {{
                let r = (|| -> jni::errors::Result<()> { $body })();
                match r {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        log::warn!("震动降级: {e}");
                        let _ = env.exception_clear();
                    }
                }
            }};
        }

        // 一级：波形 + VibrationAttributes(USAGE_ALARM)，API 31+
        if sdk >= 31 {
            try_vibrate!({
                let arr = env.new_long_array(4)?;
                env.set_long_array_region(&arr, 0, &[0, half, 150, half])?;
                let eff = env.call_static_method("android/os/VibrationEffect", "createWaveform", "([J)Landroid/os/VibrationEffect;", &[
                    JValue::Object(&arr),
                ])?
                .l()?;
                let usage = env.get_static_field("android/os/VibrationAttributes", "USAGE_ALARM", "I")?.i()?;
                let attr = env.call_static_method("android/os/VibrationAttributes", "createForUsage", "(I)Landroid/os/VibrationAttributes;", &[
                    JValue::Int(usage),
                ])?
                .l()?;
                env.call_method(&vib, "vibrate", "(Landroid/os/VibrationEffect;Landroid/os/VibrationAttributes;)V", &[
                    JValue::Object(&eff),
                    JValue::Object(&attr),
                ])?;
                Ok(())
            });
        }

        // 二级：波形（API 26+）
        if sdk >= 26 {
            try_vibrate!({
                let arr = env.new_long_array(4)?;
                env.set_long_array_region(&arr, 0, &[0, half, 150, half])?;
                let eff = env.call_static_method("android/os/VibrationEffect", "createWaveform", "([J)Landroid/os/VibrationEffect;", &[
                    JValue::Object(&arr),
                ])?
                .l()?;
                env.call_method(&vib, "vibrate", "(Landroid/os/VibrationEffect;)V", &[JValue::Object(&eff)])?;
                Ok(())
            });
        }

        // 三级：单次震动 + AudioAttributes(USAGE_ALARM)，API 26+
        if sdk >= 26 {
            try_vibrate!({
                let eff = env.call_static_method("android/os/VibrationEffect", "createOneShot", "(JI)Landroid/os/VibrationEffect;", &[
                    JValue::Long(ms as i64),
                    JValue::Int(-1),
                ])?
                .l()?;
                let usage = env.get_static_field("android/media/AudioAttributes", "USAGE_ALARM", "I")?.i()?;
                let content_type = env.get_static_field("android/media/AudioAttributes", "CONTENT_TYPE_SONIFICATION", "I")?.i()?;
                let builder = env.new_object("android/media/AudioAttributes$Builder", "()V", &[])?;
                env.call_method(&builder, "setUsage", "(I)Landroid/media/AudioAttributes$Builder;", &[
                    JValue::Int(usage),
                ])?;
                env.call_method(&builder, "setContentType", "(I)Landroid/media/AudioAttributes$Builder;", &[
                    JValue::Int(content_type),
                ])?;
                let audio = env.call_method(&builder, "build", "()Landroid/media/AudioAttributes;", &[])?.l()?;
                env.call_method(&vib, "vibrate", "(Landroid/os/VibrationEffect;Landroid/media/AudioAttributes;)V", &[
                    JValue::Object(&eff),
                    JValue::Object(&audio),
                ])?;
                Ok(())
            });
        }

        // 四级：传统 vibrate(long)
        try_vibrate!({
            env.call_method(&vib, "vibrate", "(J)V", &[JValue::Long(ms as i64)])?;
            Ok(())
        });
        Ok(())
    });
}

pub fn wake_screen() {
    with_jvm(|env, ctx| {
        let pm = get_service(env, ctx, "power")?;
        if pm.is_null() {
            return Ok(());
        }
        // 常量虽 @Deprecated 但在全部支持版本上可用；配合全屏意图双保险
        let flags = env.get_static_field("android/os/PowerManager", "SCREEN_BRIGHT_WAKE_LOCK", "I")?.i()?
            | env.get_static_field("android/os/PowerManager", "ACQUIRE_CAUSES_WAKEUP", "I")?.i()?;
        let tag = env.new_string("alertapp:screen")?;
        let wl = env.call_method(&pm, "newWakeLock", "(ILjava/lang/String;)Landroid/os/PowerManager$WakeLock;", &[
            JValue::Int(flags),
            JValue::Object(&tag),
        ])?
        .l()?;
        env.call_method(&wl, "acquire", "(J)V", &[JValue::Long(15_000)])?;
        Ok(())
    });
}

/// 提醒铃声种类 → 默认 URI（按优先级逐个回退）
/// kind：0=系统通知音（短促）1=系统闹钟音 2=系统来电铃
fn ring_default_uri<'local>(
    env: &mut JNIEnv<'local>,
    kind: u32,
) -> jni::errors::Result<JObject<'local>> {
    let order: &[i32] = match kind {
        1 => &[4, 2, 1], // 闹钟 → 通知 → 铃声
        2 => &[1, 2, 4], // 来电 → 通知 → 闹钟
        _ => &[2, 4, 1], // 通知（默认，短促）
    };
    for t in order {
        let uri = env.call_static_method("android/media/RingtoneManager", "getDefaultUri", "(I)Landroid/net/Uri;", &[
            JValue::Int(*t),
        ])?
        .l()?;
        if !uri.is_null() {
            return Ok(uri);
        }
    }
    Ok(JObject::null())
}

fn create_player<'local>(
    env: &mut JNIEnv<'local>,
    ctx: &JObject,
    uri: &JObject,
) -> jni::errors::Result<JObject<'local>> {
    env.call_static_method("android/media/MediaPlayer", "create", "(Landroid/content/Context;Landroid/net/Uri;)Landroid/media/MediaPlayer;", &[
        JValue::Object(ctx),
        JValue::Object(uri),
    ])?
    .l()
}

fn run_player_thread(kind: u32, secs: u64, once: bool) {
    let _ = std::thread::Builder::new().name("alertapp-ring".into()).spawn(move || {
        let actx = ndk_context::android_context();
        let Some(vm) = (unsafe { JavaVM::from_raw(actx.vm().cast::<jni::sys::JavaVM>()) }).ok() else {
            return;
        };
        let Ok(mut guard) = vm.attach_current_thread() else { return };
        let env = &mut guard;
        let context = unsafe { JObject::from_raw(actx.context().cast::<jni::sys::_jobject>()) };
        let res = (|| -> jni::errors::Result<()> {
            let uri = ring_default_uri(env, kind)?;
            if uri.is_null() {
                log::warn!("找不到系统铃声");
                return Ok(());
            }
            let mp = create_player(env, &context, &uri)?;
            if mp.is_null() {
                log::warn!("MediaPlayer 创建失败");
                return Ok(());
            }
            env.call_method(&mp, "setLooping", "(Z)V", &[JValue::Bool(u8::from(!once))])?;
            env.call_method(&mp, "start", "()V", &[])?;
            // 响一声时让声音自然播完，最长等 30 秒兜底；循环模式按配置时长截停
            std::thread::sleep(std::time::Duration::from_secs(if once { 30 } else { secs }));
            let _ = env.call_method(&mp, "stop", "()V", &[]);
            let _ = env.call_method(&mp, "release", "()V", &[]);
            Ok(())
        })();
        if let Err(e) = res {
            log::error!("响铃失败: {e}");
            let _ = env.exception_clear();
        }
    });
}

pub fn set_ring_config(kind: u32, mode: u32) {
    RING_KIND.store(kind, Ordering::Relaxed);
    RING_MODE.store(mode, Ordering::Relaxed);
}

pub fn preview_ringtone(kind: u32) {
    // 试听：播一次，最多 6 秒
    run_player_thread(kind, 6, true);
}

pub fn ring() {
    let kind = RING_KIND.load(Ordering::Relaxed);
    let mode = RING_MODE.load(Ordering::Relaxed);
    let secs = match mode {
        1 => 3,
        2 => 8,
        3 => 15,
        4 => 30,
        _ => 0,
    };
    run_player_thread(kind, secs, secs == 0);
}

/// 保活开关：持有/释放 PARTIAL_WAKE_LOCK（息屏时保持 CPU 与网络活动）。
///
/// 实现要点：WakeLock 的 Java 对象一旦被 GC，其 finalize 会自动 release
/// （实测 ACQ 后 ~40ms 即 REL）。所以由一条**常驻线程**永久 attach JVM、
/// 用本地引用持有 WakeLock 对象，并每 30 秒自检 isHeld()，掉锁自动重取。
pub fn apply_keepalive(on: bool) {
    let tx = KEEPALIVE_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let _ = std::thread::Builder::new().name("alertapp-keepalive".into()).spawn(move || {
            run_keepalive(rx);
        });
        tx
    });
    let _ = tx.send(on);
}

fn run_keepalive(rx: std::sync::mpsc::Receiver<bool>) {
    use std::sync::mpsc::RecvTimeoutError;

    let actx = ndk_context::android_context();
    let Some(vm) = (unsafe { JavaVM::from_raw(actx.vm().cast::<jni::sys::JavaVM>()) }).ok() else {
        return;
    };
    // 线程永久保持 attach（guard 活到线程结束），本地引用因此全程有效
    let Ok(mut guard) = vm.attach_current_thread() else { return };
    let env = &mut guard;
    let context = unsafe { JObject::from_raw(actx.context().cast::<jni::sys::_jobject>()) };

    let mut wl: Option<JObject> = None;
    loop {
        let next = match rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(on) => Some(on),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let r: jni::errors::Result<()> = match next {
            Some(true) => keepalive_acquire(env, &context, &mut wl),
            Some(false) => keepalive_release(env, &mut wl),
            None => keepalive_selfcheck(env, &context, &mut wl),
        };
        if let Err(e) = r {
            log::error!("保活操作失败: {e}");
            let _ = env.exception_clear();
        }
    }
}

/// 获取保活唤醒锁（若尚未持有）
fn keepalive_acquire<'local>(
    env: &mut JNIEnv<'local>,
    ctx: &JObject,
    wl: &mut Option<JObject<'local>>,
) -> jni::errors::Result<()> {
    if let Some(lock) = wl {
        let held = env.call_method(lock, "isHeld", "()Z", &[])?.z()?;
        if held {
            return Ok(());
        }
        *wl = None;
    }
    let pm = get_service(env, ctx, "power")?;
    if pm.is_null() {
        return Ok(());
    }
    let flags = env.get_static_field("android/os/PowerManager", "PARTIAL_WAKE_LOCK", "I")?.i()?;
    let tag = env.new_string("alertapp:keepalive")?;
    let lock = env.call_method(&pm, "newWakeLock", "(ILjava/lang/String;)Landroid/os/PowerManager$WakeLock;", &[
        JValue::Int(flags),
        JValue::Object(&tag),
    ])?
    .l()?;
    env.call_method(&lock, "acquire", "(J)V", &[
        // 7 天超时：传 i64::MAX 会导致 Handler.postDelayed(uptime+timeout)
        // 整数溢出、定时器立即触发 release()，锁瞬间被放掉
        JValue::Long(7 * 24 * 3600 * 1000),
    ])?;
    let held = env.call_method(&lock, "isHeld", "()Z", &[])?.z()?;
    log::info!("保活唤醒锁已持有 (isHeld={held})");
    *wl = Some(lock);
    Ok(())
}

/// 释放保活唤醒锁
fn keepalive_release(env: &mut JNIEnv, wl: &mut Option<JObject>) -> jni::errors::Result<()> {
    if let Some(lock) = wl.take() {
        let _ = env.call_method(&lock, "release", "()V", &[]);
        log::info!("已释放保活唤醒锁");
    }
    Ok(())
}

/// 每 30 秒自检：应为持锁状态，被系统/GC 掉锁则重新获取
fn keepalive_selfcheck<'local>(
    env: &mut JNIEnv<'local>,
    ctx: &JObject,
    wl: &mut Option<JObject<'local>>,
) -> jni::errors::Result<()> {
    if let Some(lock) = wl {
        let held = env.call_method(lock, "isHeld", "()Z", &[])?.z()?;
        if !held {
            log::warn!("保活唤醒锁意外丢失，重新获取");
            *wl = None;
            return keepalive_acquire(env, ctx, wl);
        }
    }
    Ok(())
}

fn start_activity(env: &mut JNIEnv, ctx: &JObject, intent: &JObject) -> jni::errors::Result<()> {
    let new_task = env.get_static_field("android/content/Intent", "FLAG_ACTIVITY_NEW_TASK", "I")?.i()?;
    env.call_method(intent, "setFlags", "(I)Landroid/content/Intent;", &[JValue::Int(new_task)])?;
    env.call_method(ctx, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(intent)])?;
    Ok(())
}

/// 尽力查询本应用是否已在电池优化白名单中。
/// 部分 ROM 缺失该方法（NoSuchMethodError）→ 返回 None（状态未知）。
pub fn is_battery_ignored() -> Option<bool> {
    with_jvm(|env, ctx| {
        let pkg = package_name(env, ctx)?;
        let am = get_service(env, ctx, "activity")?;
        if am.is_null() {
            return Ok(None);
        }
        let jpkg = env.new_string(&pkg)?;
        let ignoring = env.call_method(&am, "isIgnoringBatteryOptimizations", "(Ljava/lang/String;)Z", &[
            JValue::Object(&jpkg),
        ])?
        .z()?;
        Ok(Some(ignoring))
    })
    .flatten()
}

pub fn open_battery_settings() {
    let Some(pkg) = with_jvm(|env, ctx| Ok(package_name(env, ctx)?)) else { return; };
    if pkg.is_empty() {
        return;
    }

    // 一级：直接弹系统确认对话框（需 REQUEST_IGNORE_BATTERY_OPTIMIZATIONS 权限）。
    // 注意：不要在这里先查 isIgnoringBatteryOptimizations——部分 ROM 缺失该方法
    // （NoSuchMethodError），且查询失败不应阻断白名单流程。
    let direct = with_jvm(|env, ctx| {
        let action = env.new_string("android.settings.REQUEST_IGNORE_BATTERY_OPTIMIZATIONS")?;
        let data = env.new_string(&format!("package:{pkg}"))?;
        let uri = env.call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[
            JValue::Object(&data),
        ])?
        .l()?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;Landroid/net/Uri;)V", &[
            JValue::Object(&action),
            JValue::Object(&uri),
        ])?;
        start_activity(env, ctx, &intent)?;
        Ok(())
    });
    if direct.is_some() {
        return;
    }

    // 三级：打开电池优化白名单列表页（无权限要求，用户手动找到本应用开启）
    let listed = with_jvm(|env, ctx| {
        let action = env.new_string("android.settings.IGNORE_BATTERY_OPTIMIZATION_SETTINGS")?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[
            JValue::Object(&action),
        ])?;
        start_activity(env, ctx, &intent)?;
        Ok(())
    });
    if listed.is_some() {
        return;
    }

    // 四级：打开应用详情页（用户从「电池」入口自行设置）
    with_jvm(|env, ctx| {
        let action = env.new_string("android.settings.APPLICATION_DETAILS_SETTINGS")?;
        let data = env.new_string(&format!("package:{pkg}"))?;
        let uri = env.call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[
            JValue::Object(&data),
        ])?
        .l()?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;Landroid/net/Uri;)V", &[
            JValue::Object(&action),
            JValue::Object(&uri),
        ])?;
        start_activity(env, ctx, &intent)?;
        Ok(())
    });
}

pub fn open_notif_settings() {
    with_jvm(|env, ctx| {
        let pkg = package_name(env, ctx)?;
        let action = env.new_string("android.settings.APP_NOTIFICATION_SETTINGS")?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[
            JValue::Object(&action),
        ])?;
        let extra_key = env.new_string("android.provider.extra.APP_PACKAGE")?;
        let extra_val = env.new_string(&pkg)?;
        env.call_method(&intent, "putExtra", "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;", &[
            JValue::Object(&extra_key),
            JValue::Object(&extra_val),
        ])?;
        let new_task = env.get_static_field("android/content/Intent", "FLAG_ACTIVITY_NEW_TASK", "I")?.i()?;
        env.call_method(&intent, "setFlags", "(I)Landroid/content/Intent;", &[JValue::Int(new_task)])?;
        env.call_method(ctx, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    });
}

pub fn config_dir() -> PathBuf {
    with_jvm(|env, ctx| {
        let dir = env.call_method(ctx, "getFilesDir", "()Ljava/io/File;", &[])?.l()?;
        if dir.is_null() {
            return Ok(PathBuf::from("."));
        }
        let path = env.call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?.l()?;
        Ok(PathBuf::from(to_rust(env, &JString::from(path))?))
    })
    .unwrap_or_else(|| PathBuf::from("."))
}
