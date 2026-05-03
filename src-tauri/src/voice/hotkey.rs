
use tauri::{AppHandle, Emitter};

#[cfg(target_os = "macos")]
mod macos_raw {
    use std::os::raw::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tauri::{AppHandle, Emitter};

    const KCG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
    const KCG_EVENT_KEY_DOWN: u32 = 10;
    const KCG_EVENT_KEY_UP: u32 = 11;
    const KCG_HID_EVENT_TAP: u32 = 0;
    const KCG_HEAD_INSERT_EVENT_TAP: u32 = 0;
    const KCG_EVENT_TAP_OPTION_LISTEN_ONLY: u32 = 1;
    const FN_KEYCODE: i64 = 179;
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

        let keycode = CGEventGetIntegerValueField(event, KCG_KEYBOARD_EVENT_KEYCODE);

        let ctx = &*(user_info as *const HotkeyCtx);
        let active = ctx.is_active.load(Ordering::SeqCst);

        // Escape during recording → cancel
        if keycode == ESC_KEYCODE && etype == KCG_EVENT_KEY_DOWN && active {
            if ctx.is_active.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                eprintln!("[hotkey] escape pressed, cancelling recording");
                let _ = ctx.app.emit("voice-cancel", ());
            }
            return event;
        }

        if keycode != FN_KEYCODE {
            return event;
        }

        // Only act on key-down (toggle on each press)
        if etype != KCG_EVENT_KEY_DOWN {
            return event;
        }

        if !active {
            // Start recording
            if ctx.is_active.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                crate::voice::record::pre_start();
                eprintln!("[hotkey] fn-key-down (toggle ON)");
                let _ = ctx.app.emit("fn-key-down", ());
            }
        } else {
            // Stop recording
            if ctx.is_active.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                eprintln!("[hotkey] fn-key-up (toggle OFF)");
                let _ = ctx.app.emit("fn-key-up", ());
            }
        }

        event
    }

    pub fn spawn_hotkey_thread(app: AppHandle) {
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
                        let _ = Box::from_raw(ctx_ptr as *mut HotkeyCtx);
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

                    eprintln!("[hotkey] listening for fn key via CGEventTap (toggle mode)...");
                    CFRunLoopRun();

                    let _ = Box::from_raw(ctx_ptr as *mut HotkeyCtx);
                }
            })
            .expect("failed to spawn hotkey thread");
    }
}

#[cfg(target_os = "macos")]
pub fn spawn_hotkey_listener(app: AppHandle) {
    macos_raw::spawn_hotkey_thread(app);
}

#[cfg(target_os = "macos")]
pub fn check_accessibility(app: &AppHandle) {
    let ok = std::process::Command::new("osascript")
        .args(["-e", "tell application \"System Events\" to return true"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !ok {
        eprintln!("[hotkey] Accessibility permission missing");
        let _ = app.emit("accessibility-permission-required", ());
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

    pub fn spawn_hotkey_thread(app: AppHandle) {
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
pub fn spawn_hotkey_listener(app: AppHandle) {
    rdev_impl::spawn_hotkey_thread(app);
}

#[cfg(not(target_os = "macos"))]
pub fn check_accessibility(_app: &AppHandle) {}
