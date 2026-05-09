use tauri::AppHandle;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

/// Global flag: when true, the main hotkey listener ignores keys (capture in progress)
static CAPTURING: AtomicBool = AtomicBool::new(false);

/// Global hotkey code — updatable at runtime without restart
static HOTKEY_CODE: AtomicI64 = AtomicI64::new(60);

/// Tracks whether a modifier-key hotkey (e.g. RightShift) is currently held down
/// Prevents the release (flags changed) from toggling recording off immediately
static MODIFIER_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Channel for capture_hotkey — main tap sends captured keycode here
static CAPTURE_TX: std::sync::Mutex<Option<std::sync::mpsc::Sender<i64>>> = std::sync::Mutex::new(None);

/// Captured key stored by CGEventTap callback, read by poll_capture command.
/// -1 = not captured yet. Set to keycode on capture, reset to -1 by start_capture.
static CAPTURED_KEY: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

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
    const KCG_EVENT_FLAGS_CHANGED: u32 = 12;
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

        fn CFRelease(cf: *mut c_void);
        fn CFMachPortInvalidate(port: CFMachPortRef);
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
        if etype != KCG_EVENT_KEY_DOWN && etype != KCG_EVENT_KEY_UP && etype != KCG_EVENT_FLAGS_CHANGED {
            return event;
        }

        // During hotkey capture: intercept the first key pressed
        if super::CAPTURING.load(Ordering::SeqCst) {
            if etype == KCG_EVENT_KEY_DOWN || etype == KCG_EVENT_FLAGS_CHANGED {
                let kc = CGEventGetIntegerValueField(event, KCG_KEYBOARD_EVENT_KEYCODE);
                eprintln!("[capture] event: etype={}, kc={}", etype, kc);
                // Skip FLAGS_CHANGED for non-modifier keys (fn/globe key fires both,
                // prefer the KEY_DOWN which has the canonical keycode 179)
                if etype == KCG_EVENT_FLAGS_CHANGED && !(kc == 55 || kc == 56 || kc == 57 || kc == 58 || kc == 59 || kc == 60) {
                    return event;
                }
                if kc != ESC_KEYCODE {
                    super::CAPTURING.store(false, Ordering::SeqCst);
                    // If we captured a modifier key via FLAGS_CHANGED, mark it as held
                    // so the subsequent release event doesn't trigger a false toggle
                    if etype == KCG_EVENT_FLAGS_CHANGED {
                        super::MODIFIER_HELD.store(true, Ordering::SeqCst);
                    }
                    // Store in atomic for poll_capture AND emit event
                    // (belt & suspenders — poll is the primary path)
                    super::CAPTURED_KEY.store(kc, Ordering::SeqCst);
                    let name = keycode_to_name(kc);
                    let ctx = &*(user_info as *const HotkeyCtx);
                    let _ = ctx.app.emit("hotkey-captured", (kc, name));
                    eprintln!("[capture] keycode={} stored + emitted", kc);
                } else {
                    // Escape cancels capture
                    super::CAPTURING.store(false, Ordering::SeqCst);
                    super::CAPTURED_KEY.store(-1, Ordering::SeqCst);
                    let ctx = &*(user_info as *const HotkeyCtx);
                    let _ = ctx.app.emit("hotkey-captured", (-1i64, "escape"));
                    eprintln!("[capture] escape pressed, cancelled");
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

        if etype != KCG_EVENT_KEY_DOWN && etype != KCG_EVENT_FLAGS_CHANGED {
            return event;
        }

        if etype == KCG_EVENT_KEY_DOWN {
            // Regular key (fn/globe) — toggle on press
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
        } else if etype == KCG_EVENT_FLAGS_CHANGED {
            // Skip keys that also fire KEY_DOWN (e.g. fn/globe key = 179)
            // These are already handled above by the KEY_DOWN path
            if keycode == 179 {
                return event;
            }
            // Modifier key (Shift/Ctrl/Cmd/Option) — only toggle on press, ignore release
            let held = super::MODIFIER_HELD.load(Ordering::SeqCst);
            if !held {
                // This is a press event (first flags changed for this key)
                super::MODIFIER_HELD.store(true, Ordering::SeqCst);
                if !active {
                    if ctx.is_active.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        crate::voice::record::pre_start();
                        crate::commands::chat::stop_audio_queue();
                        eprintln!("[hotkey] modifier-down (toggle ON)");
                        let _ = ctx.app.emit("fn-key-down", ());
                    }
                } else {
                    if ctx.is_active.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        eprintln!("[hotkey] modifier-down (toggle OFF)");
                        let _ = ctx.app.emit("fn-key-up", ());
                    }
                }
            } else {
                // This is a release event — don't toggle, just mark as released
                super::MODIFIER_HELD.store(false, Ordering::SeqCst);
            }
        }

        event
    }

    /// Create the CGEventTap synchronously at app startup (pays kernel init cost),
    /// then spawn a run loop thread to process events.
    ///
    /// IMPORTANT: macOS 15+ only allows ONE CGEventTap per process. This function
    /// is the ONLY place in the app that creates a tap. We MUST NOT create a separate
    /// "prewarm" tap — the main tap itself IS the warmup.
    ///
    /// WHY this is done synchronously:
    ///   CGEventTapCreate() has kernel-side initialization overhead (~100-500ms on first call).
    ///   If we created the tap inside the background thread (as was done before), the tap
    ///   might not be ready when the user presses the hotkey, causing the first ~1 second
    ///   of speech to be lost (the "warmup penalty"). Creating it synchronously during
    ///   app startup pays this cost once, so the tap is always warm when used.
    pub fn spawn_hotkey_thread(app: AppHandle, hotkey_code: i64) {
        // Store initial hotkey code in global
        super::HOTKEY_CODE.store(hotkey_code, Ordering::SeqCst);

        // Create context and tap synchronously — pays the kernel init cost here
        // so the event pipeline is warm when the user first presses the hotkey.
        let ctx = Box::new(HotkeyCtx {
            app,
            is_active: Arc::new(AtomicBool::new(false)),
        });
        let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

        let event_mask =
            (1u64 << KCG_EVENT_KEY_DOWN) | (1u64 << KCG_EVENT_KEY_UP) | (1u64 << KCG_EVENT_FLAGS_CHANGED);

        let tap = unsafe {
            CGEventTapCreate(
                KCG_HID_EVENT_TAP,
                KCG_HEAD_INSERT_EVENT_TAP,
                KCG_EVENT_TAP_OPTION_LISTEN_ONLY,
                event_mask,
                cg_event_callback,
                ctx_ptr,
            )
        };

        if tap.is_null() {
            unsafe {
                eprintln!("[hotkey] CGEventTapCreate failed - is Accessibility enabled?");
                let recovered = Box::from_raw(ctx_ptr as *mut HotkeyCtx);
                let _ = recovered.app.emit("accessibility-permission-required", ());
            }
            return;
        }

        // Tap created successfully — spawn run loop thread to process events.
        // The tap is already warm; just attach it to the thread's run loop.
        // Cast raw ptrs to usize to satisfy Send bound (*mut c_void is !Send)
        let tap_val = tap as usize;
        let ctx_ptr_val = ctx_ptr as usize;
        std::thread::Builder::new()
            .name("hotkey-listener".to_string())
            .spawn(move || {
                let tap = tap_val as *mut c_void;
                let ctx_ptr = ctx_ptr_val as *mut c_void;
                unsafe {
                    let source =
                        CFMachPortCreateRunLoopSource(std::ptr::null_mut(), tap, 0);
                    if source.is_null() {
                        eprintln!("[hotkey] CFMachPortCreateRunLoopSource failed");
                        CFMachPortInvalidate(tap);
                        CFRelease(tap as *mut c_void);
                        let _ = Box::from_raw(ctx_ptr as *mut HotkeyCtx);
                        return;
                    }

                    let rl = CFRunLoopGetCurrent();
                    CFRunLoopAddSource(rl, source, kCFRunLoopCommonModes.0);

                    eprintln!("[hotkey] listening for hotkey (code={}) via CGEventTap...", hotkey_code);
                    CFRunLoopRun();

                    // Run loop exited — cleanup
                    let _ = Box::from_raw(ctx_ptr as *mut HotkeyCtx);
                }
            })
            .expect("failed to spawn hotkey thread");
    }

    pub fn capture_one_key(timeout_secs: u64) -> Result<(i64, String), String> {
        let (tx, rx) = mpsc::channel();
        *super::CAPTURE_TX.lock().unwrap() = Some(tx);

        super::CAPTURING.store(true, Ordering::SeqCst);
        eprintln!("[capture] waiting for key (timeout={}s)...", timeout_secs);

        let result = match rx.recv_timeout(std::time::Duration::from_secs(timeout_secs)) {
            Ok(keycode) if keycode < 0 => {
                eprintln!("[capture] escape received");
                Err("accessibility_permission_required".to_string())
            }
            Ok(keycode) => {
                let name = keycode_to_name(keycode);
                eprintln!("[capture] SUCCESS: keycode={} name={}", keycode, name);
                Ok((keycode, name))
            }
            Err(_) => {
                super::CAPTURING.store(false, Ordering::SeqCst);
                eprintln!("[capture] TIMEOUT after {}s", timeout_secs);
                Err("timeout".to_string())
            }
        };

        result
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
            // Specials
            36 => "Return".into(), 48 => "Tab".into(), 49 => "Space".into(),
            51 => "Delete".into(), 53 => "Escape".into(),
            50 => "`".into(), 27 => "-".into(), 24 => "=".into(),
            33 => "[".into(), 30 => "]".into(), 42 => "\\\\".into(),
            41 => ";".into(), 39 => "'".into(), 43 => ",".into(), 47 => ".".into(), 44 => "/".into(),
            // Modifiers
            55 => "Cmd".into(), 56 => "Shift".into(), 58 => "Option".into(),
            59 => "Control".into(), 57 => "CapsLock".into(),
            // Function keys
            122 => "F1".into(), 120 => "F2".into(), 99 => "F3".into(), 118 => "F4".into(),
            96 => "F5".into(), 97 => "F6".into(), 98 => "F7".into(), 100 => "F8".into(),
            101 => "F9".into(), 109 => "F10".into(), 103 => "F11".into(), 111 => "F12".into(),
            105 => "F13".into(), 107 => "F14".into(), 113 => "F15".into(),
            // Navigation
            123 => "Left".into(), 124 => "Right".into(), 125 => "Down".into(), 126 => "Up".into(),
            116 => "PageUp".into(), 121 => "PageDown".into(), 119 => "End".into(),
            115 => "Home".into(),
            // Special
            60 => "RightShift".into(),
            179 => "fn".into(),
            _ => "".into(),
        }
    }

    /// Check accessibility permissions — no-op on macOS 15+.
    /// The single CGEventTap is created by spawn_hotkey_thread, which handles
    /// the null-tap case and emits the permission-required event itself.
    /// We must NOT create a probe tap here: macOS 15+ enforces a one-tap-per-process
    /// limit, and even a released tap may not free the slot immediately.
    pub fn check_accessibility(_app: &AppHandle) {}
}

#[cfg(target_os = "macos")]
pub fn spawn_hotkey_listener(app: AppHandle, hotkey_code: i64) {
    macos_raw::spawn_hotkey_thread(app, hotkey_code);
}

/// Start capture mode — returns immediately.
/// The CGEventTap callback emits `hotkey-captured` event when a key is pressed.
/// Frontend listens for the event instead of awaiting a blocking RPC call.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn start_capture() {
    CAPTURED_KEY.store(-1, Ordering::SeqCst);
    CAPTURING.store(true, Ordering::SeqCst);
    eprintln!("[capture] capture mode activated, waiting for key...");
}

/// Legacy blocking capture — kept for non-macOS fallback.
/// macOS uses event-driven capture (start_capture + hotkey-captured event).
/// Poll for captured key — returns immediately (non-blocking).
/// Frontend calls this every ~80ms. Returns the keycode + name once captured.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn poll_capture() -> Option<(i64, String)> {
    let kc = CAPTURED_KEY.load(Ordering::SeqCst);
    if kc >= 0 {
        CAPTURING.store(false, Ordering::SeqCst);
        let name = keycode_name(kc).to_string();
        Some((kc, name))
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub fn capture_hotkey() -> Result<(i64, String), String> {
    CAPTURING.store(true, Ordering::SeqCst);
    let result = macos_raw::capture_one_key(5);
    CAPTURING.store(false, Ordering::SeqCst);
    result
}

fn keycode_name(code: i64) -> &'static str {
    match code {
        0 => "A", 1 => "S", 2 => "D", 3 => "F",
        4 => "H", 5 => "G", 6 => "Z", 7 => "X",
        8 => "C", 9 => "V", 11 => "B", 12 => "Q",
        13 => "W", 14 => "E", 15 => "R", 16 => "Y",
        17 => "T", 31 => "O", 32 => "U", 34 => "I",
        35 => "P", 37 => "L", 38 => "J", 40 => "K",
        45 => "N", 46 => "M",
        18 => "1", 19 => "2", 20 => "3", 21 => "4",
        23 => "5", 22 => "6", 26 => "7", 28 => "8",
        25 => "9", 29 => "0",
        36 => "Return", 48 => "Tab", 49 => "Space",
        51 => "Delete", 53 => "Escape",
        50 => "`", 27 => "-", 24 => "=",
        33 => "[", 30 => "]", 42 => "\\\\",
        41 => ";", 39 => "'", 43 => ",", 47 => ".", 44 => "/",
        55 => "Cmd", 56 => "Shift", 58 => "Option",
        59 => "Control", 57 => "CapsLock",
        122 => "F1", 120 => "F2", 99 => "F3", 118 => "F4",
        96 => "F5", 97 => "F6", 98 => "F7", 100 => "F8",
        101 => "F9", 109 => "F10", 103 => "F11", 111 => "F12",
        105 => "F13", 107 => "F14", 113 => "F15",
        123 => "Left", 124 => "Right", 125 => "Down", 126 => "Up",
        116 => "PageUp", 121 => "PageDown", 119 => "End",
        115 => "Home",
        60 => "RightShift",
        179 => "fn",
        _ => "",
    }
}

/// Update the active hotkey code at runtime — no restart needed
/// Returns the display name for the new keycode (e.g. "RightShift", "fn")
#[tauri::command]
pub fn update_hotkey(code: i64) -> String {
    let old = HOTKEY_CODE.load(Ordering::SeqCst);
    HOTKEY_CODE.store(code, Ordering::SeqCst);
    let old_name = keycode_name(old);
    let new_name = keycode_name(code);
    if !old_name.is_empty() && !new_name.is_empty() {
        eprintln!("[hotkey] hotkey updated: {} [{}] -> {} [{}]", old, old_name, code, new_name);
    } else {
        eprintln!("[hotkey] hotkey updated: {} -> {}", old, code);
    }
    new_name.to_string()
}

#[cfg(target_os = "macos")]
pub fn check_accessibility(app: &AppHandle) {
    macos_raw::check_accessibility(app);
}

// ─── Non-macOS platform support ─────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
mod rdev_impl {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tauri::{AppHandle, Emitter};

    struct HotkeyState {
        app: AppHandle,
        is_active: Arc<AtomicBool>,
    }

    pub fn spawn_hotkey_thread(app: AppHandle) {
        std::thread::Builder::new()
            .name("hotkey-listener".to_string())
            .spawn(move || {
                let state = Arc::new(HotkeyState {
                    app,
                    is_active: Arc::new(AtomicBool::new(false)),
                });

                let callback = move |event: rdev::Event| {
                    if let rdev::EventType::KeyPress(key) = event.event_type {
                        let code = key as i64;
                        if code == super::HOTKEY_CODE.load(Ordering::SeqCst) {
                            let s = state.clone();
                            if !s.is_active.load(Ordering::SeqCst) {
                                s.is_active.store(true, Ordering::SeqCst);
                                crate::voice::record::pre_start();
                                crate::commands::chat::stop_audio_queue();
                                let _ = s.app.emit("fn-key-down", ());
                            } else {
                                s.is_active.store(false, Ordering::SeqCst);
                                let _ = s.app.emit("fn-key-up", ());
                            }
                        } else if code == 53 {
                            // Escape
                            if state.is_active.load(Ordering::SeqCst) {
                                state.is_active.store(false, Ordering::SeqCst);
                                crate::commands::chat::stop_audio_queue();
                                let _ = state.app.emit("voice-cancel", ());
                            }
                        }
                    }
                };

                match rdev::listen(callback) {
                    Ok(()) => {}
                    Err(e) => eprintln!("[hotkey] rdev listen error: {:?}", e),
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
#[tauri::command]
pub fn capture_hotkey() -> Result<(i64, String), String> {
    Err("capture not supported on this platform".to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn check_accessibility(_app: &AppHandle) {}
