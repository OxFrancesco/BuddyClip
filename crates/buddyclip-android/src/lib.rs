use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};

use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong, jstring, JNI_FALSE, JNI_TRUE};
use log::{error, info, warn};
use serde::Serialize;
use buddyclip_core::clipboard::ClipItem;
use buddyclip_core::client::{ClientConfig, ClientEvent, SyncClientHandle, spawn_sync_client};
use buddyclip_core::crypto::MasterKey;
use buddyclip_core::pairing::PairingBundle;
use buddyclip_core::tailnet::{TailnetPolicy, is_tailscale_ip};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

struct RunningEngine {
    handle: SyncClientHandle,
    device_id: String,
    device_name: String,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static ENGINE: OnceLock<Mutex<Option<RunningEngine>>> = OnceLock::new();

fn init_logging() {
    #[cfg(target_os = "android")]
    {
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| {
            android_logger::init_once(
                android_logger::Config::default()
                    .with_max_level(log::LevelFilter::Info)
                    .with_tag("buddyclip-rust"),
            );
        });
    }
}

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("buddyclip-rt")
            .build()
            .expect("failed to build tokio runtime")
    })
}

fn engine_lock() -> &'static Mutex<Option<RunningEngine>> {
    ENGINE.get_or_init(|| Mutex::new(None))
}

fn get_jstring(env: &mut JNIEnv, s: JString) -> Result<String, String> {
    env.get_string(&s)
        .map(|js| js.into())
        .map_err(|e| format!("failed reading JNI string: {e}"))
}

fn new_jstring_or_null(env: &mut JNIEnv, s: Option<String>) -> jstring {
    match s {
        Some(text) => env
            .new_string(text)
            .map(|js| js.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

fn dispatch_event(vm: &JavaVM, callback: &GlobalRef, event: ClientEvent) {
    let mut env = match vm.attach_current_thread() {
        Ok(e) => e,
        Err(e) => {
            error!("JNI attach_current_thread failed: {e}");
            return;
        }
    };

    match event {
        ClientEvent::Connecting { target } => {
            call_state(&mut env, callback, "connecting", &target, "");
        }
        ClientEvent::Connected {
            peer_name,
            peer_addr,
            key_fingerprint,
        } => {
            let detail = format!("{peer_name} ({peer_addr})");
            call_state(&mut env, callback, "connected", &detail, &key_fingerprint);
        }
        ClientEvent::Disconnected { reason } => {
            call_state(&mut env, callback, "disconnected", &reason, "");
        }
        ClientEvent::Error(msg) => {
            call_state(&mut env, callback, "error", &msg, "");
        }
        ClientEvent::ClipSent { clip_id } => {
            if let Ok(jid) = env.new_string(&clip_id)
                && let Err(e) = env.call_method(
                    callback.as_obj(),
                    "onClipSent",
                    "(Ljava/lang/String;)V",
                    &[JValue::Object(&jid)],
                )
            {
                warn!("onClipSent JNI callback error: {e}");
                let _ = env.exception_clear();
            }
        }
        ClientEvent::HistoryCleared { cleared_at_ms } => {
            if let Err(e) = env.call_method(
                callback.as_obj(),
                "onHistoryCleared",
                "(J)V",
                &[JValue::Long(cleared_at_ms as i64)],
            ) {
                warn!("onHistoryCleared JNI callback error: {e}");
                let _ = env.exception_clear();
            }
        }
        ClientEvent::ClipReceived(item) => {
            let res = (|| -> jni::errors::Result<()> {
                let jid = env.new_string(&item.meta.id)?;
                let jorigin = env.new_string(&item.meta.origin_device_name)?;
                let jmime = env.new_string(&item.meta.mime_type)?;
                let jbytes = env.byte_array_from_slice(&item.payload)?;
                env.call_method(
                    callback.as_obj(),
                    "onClipReceived",
                    "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[BJ)V",
                    &[
                        JValue::Object(&jid),
                        JValue::Object(&jorigin),
                        JValue::Object(&jmime),
                        JValue::Object(&jbytes),
                        JValue::Long(item.meta.created_at_ms as i64),
                    ],
                )?;
                Ok(())
            })();
            if let Err(e) = res {
                warn!("onClipReceived JNI callback error: {e}");
                let _ = env.exception_clear();
            }
        }
    }
}

fn call_state(env: &mut JNIEnv, callback: &GlobalRef, state: &str, detail: &str, fp: &str) {
    let res = (|| -> jni::errors::Result<()> {
        let jstate = env.new_string(state)?;
        let jdetail = env.new_string(detail)?;
        let jfp = env.new_string(fp)?;
        env.call_method(
            callback.as_obj(),
            "onStateChanged",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V",
            &[
                JValue::Object(&jstate),
                JValue::Object(&jdetail),
                JValue::Object(&jfp),
            ],
        )?;
        Ok(())
    })();
    if let Err(e) = res {
        warn!("onStateChanged JNI callback error: {e}");
        let _ = env.exception_clear();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_buddyclip_app_BuddyClipNative_nativeStart(
    mut env: JNIEnv,
    _class: JClass,
    host: JString,
    port: jint,
    key_b64: JString,
    device_id: JString,
    device_name: JString,
    cleared_at_ms: jlong,
    callback: JObject,
) -> jstring {
    init_logging();

    let result = (|| -> Result<(), String> {
        let host = get_jstring(&mut env, host)?;
        let key_str = get_jstring(&mut env, key_b64)?;
        let device_id = get_jstring(&mut env, device_id)?;
        let device_name = get_jstring(&mut env, device_name)?;

        if !(1..=65535).contains(&port) {
            return Err(format!("invalid port: {port}"));
        }
        let key = MasterKey::from_base64(&key_str).map_err(|e| e.to_string())?;
        let vm = env
            .get_java_vm()
            .map_err(|e| format!("failed to get JavaVM: {e}"))?;
        let global_cb = env
            .new_global_ref(callback)
            .map_err(|e| format!("failed to create GlobalRef: {e}"))?;

        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let cfg = ClientConfig {
            host: host.clone(),
            port: port as u16,
            key,
            device_id: device_id.clone(),
            device_name: device_name.clone(),
            platform: "android".into(),
            policy: TailnetPolicy::Strict,
            initial_cleared_before_ms: cleared_at_ms.max(0) as u64,
        };

        let rt = runtime();
        let handle = {
            let _guard = rt.enter();
            spawn_sync_client(cfg, event_tx)
        };

        // Forward Rust client events to Kotlin callbacks on a dedicated blocking thread
        std::thread::Builder::new()
            .name("buddyclip-jni-cb".into())
            .spawn(move || {
                while let Some(ev) = event_rx.blocking_recv() {
                    dispatch_event(&vm, &global_cb, ev);
                }
            })
            .map_err(|e| format!("failed spawning callback thread: {e}"))?;

        let mut slot = engine_lock().lock().unwrap();
        if let Some(prev) = slot.take() {
            prev.handle.shutdown();
        }
        *slot = Some(RunningEngine {
            handle,
            device_id,
            device_name,
        });

        info!("started buddyclip Rust sync engine -> {host}:{port}");
        Ok(())
    })();

    new_jstring_or_null(&mut env, result.err())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_buddyclip_app_BuddyClipNative_nativeStop(
    _env: JNIEnv,
    _class: JClass,
) {
    init_logging();
    let mut slot = engine_lock().lock().unwrap();
    if let Some(eng) = slot.take() {
        eng.handle.shutdown();
        info!("stopped buddyclip Rust sync engine");
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_buddyclip_app_BuddyClipNative_nativePushClip(
    mut env: JNIEnv,
    _class: JClass,
    mime_type: JString,
    payload: JByteArray,
) -> jstring {
    init_logging();
    let result = (|| -> Result<(), String> {
        let mime = get_jstring(&mut env, mime_type)?;
        let bytes = env
            .convert_byte_array(payload)
            .map_err(|e| format!("failed reading payload bytes: {e}"))?;

        let slot = engine_lock().lock().unwrap();
        let Some(eng) = slot.as_ref() else {
            return Err("sync engine is not running".into());
        };
        let item = ClipItem::new(
            eng.device_id.clone(),
            eng.device_name.clone(),
            mime,
            bytes,
        )?;
        eng.handle.try_push_clip(item)?;
        Ok(())
    })();
    new_jstring_or_null(&mut env, result.err())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_buddyclip_app_BuddyClipNative_nativeTriggerReconnect(
    _env: JNIEnv,
    _class: JClass,
) {
    let slot = engine_lock().lock().unwrap();
    if let Some(eng) = slot.as_ref() {
        eng.handle.trigger_reconnect();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_buddyclip_app_BuddyClipNative_nativeRequestSync(
    _env: JNIEnv,
    _class: JClass,
) {
    let slot = engine_lock().lock().unwrap();
    if let Some(eng) = slot.as_ref() {
        eng.handle.request_sync();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_buddyclip_app_BuddyClipNative_nativeClearHistory(
    _env: JNIEnv,
    _class: JClass,
    cleared_at_ms: jlong,
) {
    let slot = engine_lock().lock().unwrap();
    if let Some(eng) = slot.as_ref() {
        eng.handle.try_clear_history(cleared_at_ms.max(0) as u64);
    }
}

#[derive(Serialize)]
struct ParsedPairingJson {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(rename = "hostName", skip_serializing_if = "Option::is_none")]
    host_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_buddyclip_app_BuddyClipNative_nativeParsePairingUri(
    mut env: JNIEnv,
    _class: JClass,
    uri: JString,
) -> jstring {
    let resp = match get_jstring(&mut env, uri) {
        Ok(raw) => match PairingBundle::from_uri(&raw) {
            Ok(b) => ParsedPairingJson {
                ok: true,
                host: Some(b.host),
                port: Some(b.port),
                key: Some(b.key.to_base64()),
                host_name: Some(b.host_name),
                fingerprint: Some(b.key.fingerprint()),
                error: None,
            },
            Err(e) => ParsedPairingJson {
                ok: false,
                host: None,
                port: None,
                key: None,
                host_name: None,
                fingerprint: None,
                error: Some(e.to_string()),
            },
        },
        Err(e) => ParsedPairingJson {
            ok: false,
            host: None,
            port: None,
            key: None,
            host_name: None,
            fingerprint: None,
            error: Some(e),
        },
    };
    let json = serde_json::to_string(&resp).unwrap_or_else(|_| r#"{"ok":false,"error":"json"}"#.into());
    new_jstring_or_null(&mut env, Some(json))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_buddyclip_app_BuddyClipNative_nativeKeyFingerprint(
    mut env: JNIEnv,
    _class: JClass,
    key_b64: JString,
) -> jstring {
    let fp = get_jstring(&mut env, key_b64)
        .ok()
        .and_then(|k| MasterKey::from_base64(&k).ok())
        .map(|k| k.fingerprint())
        .unwrap_or_default();
    new_jstring_or_null(&mut env, Some(fp))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_buddyclip_app_BuddyClipNative_nativeDerivePassphraseKey(
    mut env: JNIEnv,
    _class: JClass,
    passphrase: JString,
) -> jstring {
    let out = match get_jstring(&mut env, passphrase) {
        Ok(p) => match MasterKey::derive_from_passphrase(&p) {
            Ok(k) => k.to_base64(),
            Err(e) => format!("ERR:{e}"),
        },
        Err(e) => format!("ERR:{e}"),
    };
    new_jstring_or_null(&mut env, Some(out))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_buddyclip_app_BuddyClipNative_nativeIsTailnetHost(
    mut env: JNIEnv,
    _class: JClass,
    host: JString,
) -> jboolean {
    let valid = get_jstring(&mut env, host)
        .map(|h| {
            let trimmed = h.trim().trim_end_matches('.');
            if let Ok(ip) = trimmed.parse::<IpAddr>() {
                is_tailscale_ip(ip)
            } else {
                trimmed.ends_with(".ts.net") && trimmed.len() > 7
            }
        })
        .unwrap_or(false);
    if valid { JNI_TRUE } else { JNI_FALSE }
}
