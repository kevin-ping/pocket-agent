use tauri::AppHandle;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};

/// Global flag: when true, the main hotkey listener ignores keys (capture in progress)
static CAPTURING: AtomicBool = AtomicBool::new(false);

/// Global hotkey code — updatable at runtime without restart
static HOTKEY_CODE: AtomicI64 = AtomicI64::new(60);

/// Tracks whether a modifier-key hotkey (e.g. RightShift) is currently held down
/// Prevents the release (flags changed) from toggling recording off immediately
static MODIFIER_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// After capturing a modifier hotkey, swallow the very next FLAGS_CHANGED release event
/// so it doesn't immediately toggle recording or poison MODIFIER_HELD state.
static SUPPRESS_CAPTURE_RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// macOS CGEventTap handle stored globally so the callback can re-enable the tap
/// after kCGEventTapDisabledByTimeout / kCGEventTapDisabledByUserInput.
static EVENT_TAP_PTR: AtomicUsize = AtomicUsize::new(0);

/// When a modifier hotkey press is handled via FLAGS_CHANGED, some environments
/// (notably SSH-launched app sessions) also deliver an immediate KEY_DOWN for the
/// same physical press. Swallow that one duplicate to avoid start-then-stop.
static SUPPRESS_NEXT_KEYDOWN: AtomicBool = AtomicBool::new(false);

/// Double-click detection: timestamp of last hotkey press (in milliseconds)
static LAST_HOTKEY_PRESS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Double-click detection: threshold in milliseconds
const DOUBLE_CLICK_THRESHOLD_MS: u64 = 300;

/// Double-click to record mode: enabled/disabled
static DOUBLE_CLICK_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Captured key stored by CGEventTap callback, read by poll_capture command.
/// -1 = not captured yet. Set to keycode on capture, reset to -1 by start_capture.
static CAPTURED_KEY: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

#[cfg(target_os = "macos")]
mod macos_raw {
    use std::os::raw::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tauri::{AppHandle, Emitter};

    const KCG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
    const KCG_EVENT_KEY_DOWN: u32 = 10;
    const KCG_EVENT_KEY_UP: u32 = 11;
    const KCG_EVENT_FLAGS_CHANGED: u32 = 12;
    const KCG_HID_EVENT_TAP: u32 = 0;
    const KCG_HEAD_INSERT_EVENT_TAP: u32 = 0;
    const KCG_EVENT_TAP_OPTION_LISTEN_ONLY: u32 = 1;
    const KCG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFFFFFE;
    const KCG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFFFFFF;
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
        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
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
        if etype == KCG_EVENT_TAP_DISABLED_BY_TIMEOUT || etype == KCG_EVENT_TAP_DISABLED_BY_USER_INPUT {
            let tap = super::EVENT_TAP_PTR.load(Ordering::SeqCst) as CFMachPortRef;
            if !tap.is_null() {
                CGEventTapEnable(tap, true);
                eprintln!("[hotkey] event tap disabled by macOS, re-enabled");
            }
            return event;
        }

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
                if etype == KCG_EVENT_FLAGS_CHANGED && !(kc == 55 || kc == 56 || kc == 57 || kc == 58 || kc == 59 || kc == 60 || kc == 63) {
                    return event;
                }
                if kc != ESC_KEYCODE {
                    super::CAPTURING.store(false, Ordering::SeqCst);
                    // If we captured a modifier key via FLAGS_CHANGED, swallow the
                    // subsequent release event once. Do NOT poison MODIFIER_HELD, or the
                    // first real press of the new hotkey gets mistaken for a release.
                    if etype == KCG_EVENT_FLAGS_CHANGED {
                        super::SUPPRESS_CAPTURE_RELEASE.store(true, Ordering::SeqCst);
                    }
                    // Normalize fn key keycode: FLAGS_CHANGED reports 63, KEY_DOWN reports 179
                    // Use 179 as canonical so hotkey matching works consistently
                    let store_kc = if kc == 63 { 179 } else { kc };
                    // Store in atomic for poll_capture AND emit event
                    // (belt & suspenders — poll is the primary path)
                    super::CAPTURED_KEY.store(store_kc, Ordering::SeqCst);
                    let name = keycode_to_name(store_kc);
                    let ctx = &*(user_info as *const HotkeyCtx);
                    let _ = ctx.app.emit("hotkey-captured", (store_kc, name));
                    eprintln!("[capture] keycode={} (raw={}) stored + emitted", store_kc, kc);
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

        let mut keycode = CGEventGetIntegerValueField(event, KCG_KEYBOARD_EVENT_KEYCODE);
        // Normalize fn key: FLAGS_CHANGED reports kc=63, canonical is 179
        if keycode == 63 { keycode = 179; }

        let ctx = &*(user_info as *const HotkeyCtx);
        let active = ctx.is_active.load(Ordering::SeqCst);

        // Swallow the immediate release that follows modifier-hotkey capture even if
        // HOTKEY_CODE has not been updated to the newly captured key yet.
        if etype == KCG_EVENT_FLAGS_CHANGED && super::SUPPRESS_CAPTURE_RELEASE.swap(false, Ordering::SeqCst) {
            return event;
        }

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
            if super::SUPPRESS_NEXT_KEYDOWN.swap(false, Ordering::SeqCst) {
                return event;
            }
            // Regular key (fn/globe) — check double-click mode setting
            let double_click_mode = super::DOUBLE_CLICK_MODE.load(Ordering::SeqCst);
            
            if double_click_mode {
                // Double-click mode: detect double-click
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let last_press = super::LAST_HOTKEY_PRESS.load(Ordering::SeqCst);
                let is_double_click = (now - last_press) < super::DOUBLE_CLICK_THRESHOLD_MS && last_press > 0;
                super::LAST_HOTKEY_PRESS.store(now, Ordering::SeqCst);

                if is_double_click {
                    // Double-click: toggle recording state
                    if !active {
                        if ctx.is_active.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                            crate::voice::record::pre_start();
                            crate::commands::chat::stop_audio_queue();
                            eprintln!("[hotkey] double-click mode: start recording");
                            let _ = ctx.app.emit("fn-key-down", ());
                        }
                    } else {
                        if ctx.is_active.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                            eprintln!("[hotkey] double-click mode: stop recording");
                            let _ = ctx.app.emit("fn-key-up", ());
                        }
                    }
                }
            } else {
                // Single-click mode: toggle on every press (start/stop)
                if !active {
                    if ctx.is_active.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        crate::voice::record::pre_start();
                        crate::commands::chat::stop_audio_queue();
                        eprintln!("[hotkey] single-click mode: start recording");
                        let _ = ctx.app.emit("fn-key-down", ());
                    }
                } else {
                    if ctx.is_active.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        eprintln!("[hotkey] single-click mode: stop recording");
                        let _ = ctx.app.emit("fn-key-up", ());
                    }
                }
            }
        } else if etype == KCG_EVENT_FLAGS_CHANGED {
            // fn key (179) locally ONLY fires FLAGS_CHANGED (never KEY_DOWN),
            // so it must be handled right here in the modifier path.
            let held = super::MODIFIER_HELD.load(Ordering::SeqCst);
            if !held {
                // This is a press event (first flags changed for this key)
                super::MODIFIER_HELD.store(true, Ordering::SeqCst);
                super::SUPPRESS_NEXT_KEYDOWN.store(true, Ordering::SeqCst);
                
                let double_click_mode = super::DOUBLE_CLICK_MODE.load(Ordering::SeqCst);
                
                if double_click_mode {
                    // Double-click mode: detect double-click
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let last_press = super::LAST_HOTKEY_PRESS.load(Ordering::SeqCst);
                    let is_double_click = (now - last_press) < super::DOUBLE_CLICK_THRESHOLD_MS && last_press > 0;
                    super::LAST_HOTKEY_PRESS.store(now, Ordering::SeqCst);

                    if is_double_click {
                        // Double-click: toggle recording state
                        if !active {
                            if ctx.is_active.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                                crate::voice::record::pre_start();
                                crate::commands::chat::stop_audio_queue();
                                eprintln!("[hotkey] modifier double-click mode: start recording");
                                let _ = ctx.app.emit("fn-key-down", ());
                            }
                        } else {
                            if ctx.is_active.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                                eprintln!("[hotkey] modifier double-click mode: stop recording");
                                let _ = ctx.app.emit("fn-key-up", ());
                            }
                        }
                    }
                } else {
                    // Single-click mode: toggle on every press (start/stop)
                    if !active {
                        if ctx.is_active.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                            crate::voice::record::pre_start();
                            crate::commands::chat::stop_audio_queue();
                            eprintln!("[hotkey] modifier single-click mode: start recording");
                            let _ = ctx.app.emit("fn-key-down", ());
                        }
                    } else {
                        if ctx.is_active.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                            eprintln!("[hotkey] modifier single-click mode: stop recording");
                            let _ = ctx.app.emit("fn-key-up", ());
                        }
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

        super::EVENT_TAP_PTR.store(tap as usize, Ordering::SeqCst);

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
            63 => "fn".into(),
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
    SUPPRESS_CAPTURE_RELEASE.store(false, Ordering::SeqCst);
    SUPPRESS_NEXT_KEYDOWN.store(false, Ordering::SeqCst);
    MODIFIER_HELD.store(false, Ordering::SeqCst);
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
        let name = keycode_name(kc);
        let display = if name.is_empty() { format!("Key({})", kc) } else { name.to_string() };
        Some((kc, display))
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn keycode_name(code: i64) -> &'static str {
    // Note: returns "" for unknown keys — poll_capture converts to Key(code) fallback
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

/// Update double-click to record mode
#[tauri::command]
pub fn set_double_click_mode(enabled: bool) {
    DOUBLE_CLICK_MODE.store(enabled, Ordering::SeqCst);
    eprintln!("[hotkey] double_click_mode set to: {}", enabled);
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
