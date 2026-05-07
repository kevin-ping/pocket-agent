use tauri::{AppHandle, Emitter};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

/// Global flag: when true, the main hotkey listener ignores keys (capture in progress)
static CAPTURING: AtomicBool = AtomicBool::new(false);

/// Global hotkey code — updatable at runtime without restart
static HOTKEY_CODE: AtomicI64 = AtomicI64::new(179);

/// Channel for capture_hotkey — main tap sends captured keycode here
static CAPTURE_TX: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<i64>>> = std::sync::OnceLock::new();

#[cfg(target_os = "macos")]
mod macos_raw {
    use std::os::raw::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use tauri::{AppHandle, Emitter};

    const KCG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
    const KCG_EVENT_KEY_DOWN: u32 = 10;
    const KCG_EVENT_KEY_UP: u32 = 11;
    const KCG_HID_EVENT_TAP: u32 = 0;
    const KCG_HEAD_INSERT_EVENT_TAP: u32 = 0;
    const KCG_EVENT_TAP_OPTION_LISTEN_ONLY: u32 = 1;
    const ESC_KEYCODE: i64 = 53;

    type CGEventRef = *mut c_void;
    type CFMachPortRef = *mut c_void;
    type CFRunLoopSourceRef = *mut c_void;
    type CFRunLoopRef = *mut c_void;

    #[repr(transparent)]
    struct SyncPtr(*mut c_void);
    unsafe impl Sync for SyncPtr {}

    extern "C" {
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: u64,
            callback: unsafe extern "C" fn(
                proxy: *mut c_void,
                etype: u32,
                event: CGEventRef,
                user_info: *mut c_void,
            ) -> CGEventRef,
            user_info: *mut c_void,
        ) -> CFMachPortRef;

        fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;

        fn CFMachPortCreateRunLoopSource(
            allocator: *mut c_void,
            port: CFMachPortRef,
            order: i64,
        ) -> CFRunLoopSourceRef;

        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: *mut c_void);
        fn CFRunLoopRun();

        #[link_name = "kCFRunLoopCommonModes"]
        static kCFRunLoopCommonModes: SyncPtr;
    }

    struct HotkeyCtx {
        app: AppHandle,
        is_active: Arc<AtomicBool>,
    }

    unsafe extern "C" fn cg_event_callback(
        _proxy: *mut c_void,
        etype: u32,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef {
        if etype != KCG_EVENT_KEY_DOWN && etype != KCG_EVENT_KEY_UP {
            return event;
        }

        // During hotkey capture: intercept the first key pressed
        if super::CAPTURING.load(Ordering::SeqCst) {
            if etype == KCG_EVENT_KEY_DOWN {
                let kc = CGEventGetIntegerValueField(event, KCG_KEYBOARD_EVENT_KEYCODE);
                if kc != ESC_KEYCODE {
                    super::CAPTURING.store(false, Ordering::SeqCst);
                    if let Some(tx) = super::CAPTURE_TX.get() {
                        let _ = tx.lock().unwrap().send(kc);
                    }
                } else {
                    // Escape cancels capture
                    super::CAPTURING.store(false, Ordering::SeqCst);
                    if let Some(tx) = super::CAPTURE_TX.get() {
                        let _ = tx.lock().unwrap().send(-1);
                    }
                }
            }
            return event;
        }

        let keycode = CGEventGetIntegerValueField(event, KCG_KEYBOARD_EVENT_KEYCODE);

        let ctx = &*(user_info as *const HotkeyCtx);
        let active = ctx.is_active.load(Ordering::SeqCst);

        // Escape during recording -> cancel
        if keycode == ESC_KEYCODE && etype == KCG_EVENT_KEY_DOWN && active {
            if ctx.is_active.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                eprintln!("[hotkey] escape pressed, cancelling recording");
                crate::commands::chat::stop_audio_queue();
                let _ = ctx.app.emit("voice-cancel", ());
            }
            return event;
        }

        // Read hotkey code from global (live-updatable)
        let hotkey_code = super::HOTKEY_CODE.load(Ordering::SeqCst);
        if keycode != hotkey_code {
            return event;
        }

        if etype != KCG_EVENT_KEY_DOWN {
            return event;
        }

        if !active {
            if ctx.is_active.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                crate::voice::record::pre_start();
                crate::commands::chat::stop_audio_queue();
                eprintln!("[hotkey] hotkey-down (toggle ON)");
                let _ = ctx.app.emit("fn-key-down", ());
            }
        } else {
            if ctx.is_active.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                eprintln!("[hotkey] hotkey-up (toggle OFF)");
                let _ = ctx.app.emit("fn-key-up", ());
            }
        }

        event
    }

    pub fn spawn_hotkey_thread(app: AppHandle, hotkey_code: i64) {
        // Store initial hotkey code in global
        super::HOTKEY_CODE.store(hotkey_code, Ordering::SeqCst);

        std::thread::Builder::new()
            .name("hotkey-listener".to_string())
            .spawn(move || {
                let ctx = Box::new(HotkeyCtx {
                    app,
                    is_active: Arc::new(AtomicBool::new(false)),
                });
                let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

                let event_mask = (1u64 << KCG_EVENT_KEY_DOWN) | (1u64 << KCG_EVENT_KEY_UP);

                unsafe {
                    let tap = CGEventTapCreate(
                        KCG_HID_EVENT_TAP,
                        KCG_HEAD_INSERT_EVENT_TAP,
                        KCG_EVENT_TAP_OPTION_LISTEN_ONLY,
                        event_mask,
                        cg_event_callback,
                        ctx_ptr,
                    );

                    if tap.is_null() {
                        eprintln!("[hotkey] CGEventTapCreate failed - is Accessibility enabled?");
                        let recovered = Box::from_raw(ctx_ptr as *mut HotkeyCtx);
                        let _ = recovered.app.emit("accessibility-permission-required", ());
                        return;
                    }

                    let source =
                        CFMachPortCreateRunLoopSource(std::ptr::null_mut(), tap, 0);
                    if source.is_null() {
                        eprintln!("[hotkey] CFMachPortCreateRunLoopSource failed");
                        let _ = Box::from_raw(ctx_ptr as *mut HotkeyCtx);
                        return;
                    }

                    let rl = CFRunLoopGetCurrent();
                    CFRunLoopAddSource(rl, source, kCFRunLoopCommonModes.0);

                    eprintln!("[hotkey] listening for hotkey (code={}) via CGEventTap...", hotkey_code);
                    CFRunLoopRun();

                    let _ = Box::from_raw(ctx_ptr as *mut HotkeyCtx);
                }
            })
            .expect("failed to spawn hotkey thread");
    }

    pub fn capture_one_key(timeout_secs: u64) -> Result<(i64, String), String> {
        let (tx, rx) = mpsc::channel();
        let _ = super::CAPTURE_TX.set(std::sync::Mutex::new(tx));

        super::CAPTURING.store(true, Ordering::SeqCst);

        let result = match rx.recv_timeout(std::time::Duration::from_secs(timeout_secs)) {
            Ok(keycode) if keycode < 0 => {
                Err("accessibility_permission_required".to_string())
            }
            Ok(keycode) => {
                let name = keycode_to_name(keycode);
                Ok((keycode, name))
            }
            Err(_) => {
                super::CAPTURING.store(false, Ordering::SeqCst);
                Err("timeout".to_string())
            }
        };

        result
    }

    /// Pre-warm the CGEventTap infrastructure to avoid first-click stutter
    pub fn prewarm() {
        std::thread::Builder::new()
            .name("hotkey-prewarm".to_string())
            .spawn(|| {
                unsafe {
                    extern "C" fn noop_cb(
                        _: *mut c_void, _: u32, e: CGEventRef, _: *mut c_void,
                    ) -> CGEventRef { e }

                    let tap = CGEventTapCreate(
                        KCG_HID_EVENT_TAP,
                        KCG_HEAD_INSERT_EVENT_TAP,
                        KCG_EVENT_TAP_OPTION_LISTEN_ONLY,
                        1u64 << KCG_EVENT_KEY_DOWN,
                        noop_cb,
                        std::ptr::null_mut(),
                    );
                    // Creating it pays the OS init cost; let it leak (one-time tiny cost)
                    if tap.is_null() {
                        eprintln!("[hotkey] prewarm: CGEventTapCreate failed (accessibility?)");
                    } else {
                        eprintln!("[hotkey] prewarm: CGEventTap initialized");
                    }
                }
            })
            .ok();
    }

    fn keycode_to_name(code: i64) -> String {
        match code {
            // Letters
            0 => "A".into(), 1 => "S".into(), 2 => "D".into(), 3 => "F".into(),
            4 => "H".into(), 5 => "G".into(), 6 => "Z".into(), 7 => "X".into(),
            8 => "C".into(), 9 => "V".into(), 11 => "B".into(), 12 => "Q".into(),
            13 => "W".into(), 14 => "E".into(), 15 => "R".into(), 16 => "Y".into(),
            17 => "T".into(), 31 => "O".into(), 32 => "U".into(), 34 => "I".into(),
            35 => "P".into(), 37 => "L".into(), 38 => "J".into(), 40 => "K".into(),
            45 => "N".into(), 46 => "M".into(),
            // Numbers
            18 => "1".into(), 19 => "2".into(), 20 => "3".into(), 21 => "4".into(),
            23 => "5".into(), 22 => "6".into(), 26 => "7".into(), 28 => "8".into(),
            25 => "9".into(), 29 => "0".into(),
            // Special
            36 => "Return".into(), 48 => "Tab".into(), 49 => "Space".into(),
            51 => "Delete".into(), 53 => "Escape".into(),
            50 => "`".into(), 27 => "-".into(), 24 => "=".into(),
            33 => "[".into(), 30 => "]".into(), 42 => "\\".into(),
            41 => ";".into(), 39 => "'".into(), 43 => ",".into(),
            47 => ".".into(), 44 => "/".into(),
            // Modifiers
            55 => "Cmd".into(), 56 => "Shift".into(), 58 => "Option".into(),
            59 => "Control".into(), 57 => "CapsLock".into(),
            // Function keys
            122 => "F1".into(), 120 => "F2".into(), 99 => "F3".into(), 118 => "F4".into(),
            96 => "F5".into(), 97 => "F6".into(), 98 => "F7".into(), 100 => "F8".into(),
            101 => "F9".into(), 109 => "F10".into(), 103 => "F11".into(), 111 => "F12".into(),
            105 => "F13".into(), 107 => "F14".into(), 113 => "F15".into(),
            // Arrow keys
            123 => "Left".into(), 124 => "Right".into(), 125 => "Down".into(), 126 => "Up".into(),
            // Navigation
            116 => "PageUp".into(), 121 => "PageDown".into(), 119 => "End".into(),
            115 => "Home".into(),
            // fn key (MacBook built-in)
            179 => "fn".into(),
            _ => format!("Key({})", code),
        }
    }

    /// Test if CGEventTap can be created (true = accessibility permission granted)
    pub fn test_tap_available() -> bool {
        unsafe {
            extern "C" fn noop_cb(
                _: *mut c_void, _: u32, e: CGEventRef, _: *mut c_void,
            ) -> CGEventRef { e }

            let tap = CGEventTapCreate(
                KCG_HID_EVENT_TAP,
                KCG_HEAD_INSERT_EVENT_TAP,
                KCG_EVENT_TAP_OPTION_LISTEN_ONLY,
                1u64 << KCG_EVENT_KEY_DOWN,
                noop_cb,
                std::ptr::null_mut(),
            );
            !tap.is_null()
        }
    }
}

#[cfg(target_os = "macos")]
pub fn spawn_hotkey_listener(app: AppHandle, hotkey_code: i64) {
    macos_raw::spawn_hotkey_thread(app, hotkey_code);
}

#[cfg(target_os = "macos")]
pub fn prewarm_capture() {
    macos_raw::prewarm();
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub fn capture_hotkey() -> Result<(i64, String), String> {
    CAPTURING.store(true, Ordering::SeqCst);
    let result = macos_raw::capture_one_key(5);
    CAPTURING.store(false, Ordering::SeqCst);
    result
}

/// Update the active hotkey code at runtime — no restart needed
#[tauri::command]
pub fn update_hotkey(code: i64) {
    let old = HOTKEY_CODE.load(Ordering::SeqCst);
    HOTKEY_CODE.store(code, Ordering::SeqCst);
    eprintln!("[hotkey] hotkey updated: {} -> {}", old, code);
}

#[cfg(target_os = "macos")]
pub fn check_accessibility(app: &AppHandle) {
    let tap_ok = std::thread::Builder::new()
        .name("accessibility-check".to_string())
        .spawn(|| macos_raw::test_tap_available())
        .map(|h| h.join().unwrap_or(false))
        .unwrap_or(false);

    if !tap_ok {
        eprintln!("[hotkey] Accessibility permission missing — CGEventTap test failed");
        let _ = app.emit("accessibility-permission-required", ());
        let _ = std::process::Command::new("open")
            .args(["x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"])
            .spawn();
    } else {
        eprintln!("[hotkey] Accessibility permission OK");
    }
}

#[cfg(not(target_os = "macos"))]
mod rdev_impl {
    use rdev::{listen, Event, EventType, Key};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tauri::{AppHandle, Emitter};

    fn is_trigger_key(key: &Key) -> bool {
        matches!(key, Key::Space)
    }

    pub fn spawn_hotkey_thread(app: AppHandle, _hotkey_code: i64) {
        let is_active = Arc::new(AtomicBool::new(false));
        let ctrl_down = Arc::new(AtomicBool::new(false));

        std::thread::Builder::new()
            .name("hotkey-listener".to_string())
            .spawn(move || {
                let is_active = is_active.clone();
                let ctrl_down = ctrl_down.clone();

                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    listen(move |event: Event| {
                        match &event.event_type {
                            EventType::KeyPress(Key::ControlLeft)
                            | EventType::KeyPress(Key::ControlRight) => {
                                ctrl_down.store(true, Ordering::SeqCst);
                            }
                            EventType::KeyRelease(Key::ControlLeft)
                            | EventType::KeyRelease(Key::ControlRight) => {
                                ctrl_down.store(false, Ordering::SeqCst);
                            }
                            EventType::KeyPress(key) if is_trigger_key(key) => {
                                if !ctrl_down.load(Ordering::SeqCst) {
                                    return;
                                }
                                if is_active
                                    .compare_exchange(
                                        false,
                                        true,
                                        Ordering::SeqCst,
                                        Ordering::SeqCst,
                                    )
                                    .is_ok()
                                {
                                    let _ = app.emit("fn-key-down", ());
                                }
                            }
                            EventType::KeyRelease(key) if is_trigger_key(key) => {
                                if is_active
                                    .compare_exchange(
                                        true,
                                        false,
                                        Ordering::SeqCst,
                                        Ordering::SeqCst,
                                    )
                                    .is_ok()
                                {
                                    let _ = app.emit("fn-key-up", ());
                                }
                            }
                            _ => {}
                        }
                    })
                }));

                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => eprintln!("[hotkey] rdev listen error: {:?}", e),
                    Err(_) => eprintln!("[hotkey] rdev panicked"),
                }
            })
            .expect("failed to spawn hotkey thread");
    }
}

#[cfg(not(target_os = "macos"))]
pub fn spawn_hotkey_listener(app: AppHandle, _hotkey_code: i64) {
    rdev_impl::spawn_hotkey_thread(app);
}

#[cfg(not(target_os = "macos"))]
pub fn prewarm_capture() {}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn capture_hotkey() -> Result<(i64, String), String> {
    Err("capture not supported on this platform".to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn check_accessibility(_app: &AppHandle) {}
