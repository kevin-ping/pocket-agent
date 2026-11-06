package com.hermes.pocketagent.phone

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.app.AlertDialog
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.os.BatteryManager
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.webkit.JavascriptInterface
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast

/**
 * Two screens. The device page (native) lists the paired computers: who uses
 * the phone, who is online, 「接入」, and adding one by scanning the QR code
 * the computer shows ([QrScanner], [Pairing]). While a PA uses the phone (the occupant, decided in
 * [AudioService]) a full-screen WebView runs that PA's own frontend (`/ui/`);
 * Back returns to the device page. Audio lives in [AudioService] so it keeps
 * running with the screen off.
 */
class MainActivity : Activity() {
    private val handler = Handler(Looper.getMainLooper())
    private var webView: WebView? = null
    private var shownPa: String? = null // the PA whose page the WebView shows
    private enum class Page { DEVICES, SCANNER, PAIRING, WEB }
    private var page = Page.DEVICES // what is on screen; background refreshes may only redraw DEVICES
    private var scanner: QrScanner? = null // the 「扫码添加电脑」 page
    private var pendingScan = false // the user asked to scan, waiting for the permission / onResume
    private var camera: PhoneCamera? = null
    private var pendingCamera: CameraRequest? = null // the open the user asked for, waiting for the permission / onResume
    // A CAMERA permission request is in flight. Kept apart from pendingCamera: closing drops
    // the intent, but only the result ends the request (a second one would be cancelled at once).
    private var awaitingCameraPermission = false
    private var resumed = false
    // Bumped whenever the camera must not open from earlier requests (pause, navigation, close).
    @Volatile private var cameraGen = 0
    // The notification's 「断开」: close the camera and the page too, so /ui goes and the Mac drives again.
    private val disconnect: () -> Unit = { runOnUiThread { stopCamera(); finishAndRemoveTask() } }

    private data class CameraRequest(val left: Int, val top: Int, val width: Int, val height: Int, val front: Boolean)

    // Plugged in → keep the screen on (it doubles as a desk display).
    private var plugged = false
    private val powerReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) = keepScreenOnWhenPlugged(intent)
    }
    // A conversation is going on (the wake word or PA speaking within VOICE_WINDOW_MS):
    // the screen stays on, since Chromium freezes the page once the activity stops.
    private var voiceWindow = false
    // From AudioService: a stopped activity has to be brought forward for the page to run.
    private val voiceActive: (Boolean) -> Unit = { wake -> voiceActive(wake, bringForward = wake && !resumed) }
    // From AudioService: the phone now belongs to a PA (show its page) or to none (the device page).
    private val occupantChanged: (String?) -> Unit = { key -> route(key) }
    // A PA going online / offline must not close the scanner or the 「正在配对」 page.
    private val devicesChanged: () -> Unit = { if (page == Page.DEVICES) showDevices() }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        registerReceiver(powerReceiver, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
            ?.let { keepScreenOnWhenPlugged(it) }
        AudioService.onDisconnect = disconnect
        AudioService.onVoiceActive = voiceActive
        AudioService.onOccupant = occupantChanged
        AudioService.onChanged = devicesChanged
        start()
        if (intent.getBooleanExtra(EXTRA_WAKE, false)) voiceActive(wake = true, bringForward = false)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (intent.getBooleanExtra(EXTRA_WAKE, false)) voiceActive(wake = true, bringForward = false)
    }

    override fun onResume() {
        super.onResume()
        resumed = true
        AudioService.setForeground(true)
        if (pendingScan && !awaitingCameraPermission) showScanner()
        // Back from the CAMERA permission dialog: open only if that request still stands.
        val request = pendingCamera
        if (request != null && !awaitingCameraPermission) {
            pendingCamera = null
            startCamera(request)
        }
    }

    // The camera runs only while the app is in the foreground (also covers screen lock).
    // The permission dialog pauses us too, so its request is kept until the answer.
    override fun onPause() {
        super.onPause()
        resumed = false
        AudioService.setForeground(false)
        if (!awaitingCameraPermission) stopCamera()
        if (scanner != null) showDevices() // the camera runs only in the foreground
    }

    override fun onStop() {
        super.onStop()
        stopCamera()
        // Power button / Home: next time the lock screen shows as usual.
        setShowWhenLocked(false)
        setTurnScreenOn(false)
    }

    override fun onDestroy() {
        if (AudioService.onDisconnect === disconnect) AudioService.onDisconnect = null
        if (AudioService.onVoiceActive === voiceActive) AudioService.onVoiceActive = null
        if (AudioService.onOccupant === occupantChanged) AudioService.onOccupant = null
        if (AudioService.onChanged === devicesChanged) AudioService.onChanged = null
        scanner?.close()
        camera?.close()
        unregisterReceiver(powerReceiver)
        handler.removeCallbacksAndMessages(null)
        webView?.destroy()
        super.onDestroy()
    }

    @Deprecated("Back opens the device page; predictive back is not opted in.")
    override fun onBackPressed() {
        if (webView != null || scanner != null) showDevices() else super.onBackPressed()
    }

    private fun keepScreenOnWhenPlugged(battery: Intent) {
        plugged = battery.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) != 0
        updateKeepScreenOn()
    }

    private fun updateKeepScreenOn() {
        if (plugged || voiceWindow) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        else window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
    }

    /**
     * PA heard the wake word (`wake`) or is speaking: keep the screen on for
     * VOICE_WINDOW_MS more. The wake word also lights the screen and shows the
     * page over the lock screen (user's choice, 2026-11-05) — only for this
     * window, so the power button still leads to the lock screen.
     */
    private fun voiceActive(wake: Boolean, bringForward: Boolean) {
        if (wake) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        }
        if (bringForward) startActivity(wakeIntent(this))
        voiceWindow = true
        updateKeepScreenOn()
        handler.removeCallbacksAndMessages(VOICE)
        handler.postDelayed({
            voiceWindow = false
            updateKeepScreenOn()
            setShowWhenLocked(false)
            setTurnScreenOn(false)
        }, VOICE, VOICE_WINDOW_MS)
    }

    private fun start() {
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQUEST_MIC)
            return
        }
        // Already running: it re-reads the paired list.
        if (Config.pas(this).isNotEmpty()) AudioService.start(this)
        route(Config.occupant(this))
    }

    /** The occupant's page, or the device page when the phone is free. */
    private fun route(occupant: String?) {
        val pa = Config.pa(this, occupant)
        if (pa != null) {
            if (webView == null || shownPa != pa.key) showWebView(pa)
        } else if (page == Page.DEVICES || page == Page.WEB) showDevices() // scanning / pairing: keep it
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, results: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, results)
        if (requestCode == REQUEST_CAMERA) {
            awaitingCameraPermission = false
            if (pendingScan) {
                when {
                    results.firstOrNull() == PackageManager.PERMISSION_GRANTED -> if (resumed) showScanner() // else onResume
                    results.isEmpty() -> pendingScan = false // cancelled by the system, not refused
                    else -> {
                        pendingScan = false
                        toast("需要相机权限才能扫码添加电脑")
                    }
                }
            }
            val request = pendingCamera ?: return // dropped meanwhile (navigation / left the app)
            when {
                results.firstOrNull() == PackageManager.PERMISSION_GRANTED -> if (resumed) {
                    pendingCamera = null
                    startCamera(request)
                } // else onResume opens it
                results.isEmpty() -> { // cancelled by the system, not refused by the user
                    pendingCamera = null
                    notifyCamera(false)
                }
                else -> {
                    pendingCamera = null
                    Toast.makeText(this, "需要相机权限才能让 Pocket Agent 看", Toast.LENGTH_LONG).show()
                    notifyCamera(false)
                }
            }
            return
        }
        if (requestCode != REQUEST_MIC) return
        if (results.firstOrNull() == PackageManager.PERMISSION_GRANTED) {
            start()
        } else {
            Toast.makeText(this, "需要麦克风权限才能和 Pocket Agent 对话", Toast.LENGTH_LONG).show()
            showDevices()
        }
    }

    @SuppressLint("SetJavaScriptEnabled", "JavascriptInterface")
    private fun showWebView(config: Pa) {
        closeScanner()
        page = Page.WEB
        shownPa = config.key
        camera?.close()
        webView?.destroy()
        val view = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            // The wake / ready cues are played by the page itself.
            settings.mediaPlaybackRequiresUserGesture = false
            setBackgroundColor(0xFF0B0C18.toInt())
            webViewClient = object : WebViewClient() {
                // A reload / navigation brings up a page that shows the camera closed:
                // close it for real, the user opens it again.
                override fun onPageStarted(view: WebView, url: String?, favicon: Bitmap?) = stopCamera()

                // Mac unreachable (Wi-Fi down, PA restarting): keep retrying. Once the
                // page is up, src/lib/ipc.ts reconnects and reloads on its own.
                override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
                    if (!request.isForMainFrame) return
                    handler.removeCallbacksAndMessages(RETRY)
                    handler.postDelayed({ view.loadUrl(config.uiUrl()) }, RETRY, RETRY_MS)
                }
            }
        }
        view.addJavascriptInterface(CameraBridge(config.uiPrefix()), "PocketAgentCamera")
        val root = FrameLayout(this).apply { addView(view) }
        camera = PhoneCamera(this, root)
        webView = view
        setContentView(root)
        hideSystemBars()
        view.loadUrl(config.uiUrl())
    }

    private fun hideSystemBars() {
        window.insetsController?.let {
            it.hide(WindowInsets.Type.systemBars())
            it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
    }

    // ── Camera (FEAT-CAM1) ──────────────────────────────────────────────────

    /**
     * What the page may do with the camera: open it over a rectangle (CSS
     * pixels), flip, close. Calls arrive on a binder thread; each is checked on
     * the main thread against the page actually loaded, so only PA's own
     * frontend can drive it, and dropped if the app left the foreground or the
     * page navigated since the call.
     */
    private inner class CameraBridge(private val allowedPrefix: String) {
        @JavascriptInterface
        fun openCamera(x: Double, y: Double, width: Double, height: Double, facing: String) = onPage {
            val view = webView ?: return@onPage
            val d = resources.displayMetrics.density
            if (!listOf(x, y, width, height).all { it.isFinite() } || width <= 0 || height <= 0) return@onPage
            val left = (x * d).toInt().coerceIn(0, view.width)
            val top = (y * d).toInt().coerceIn(0, view.height)
            val w = (width * d).toInt().coerceAtMost(view.width - left)
            val h = (height * d).toInt().coerceAtMost(view.height - top)
            if (w <= 0 || h <= 0) return@onPage
            startCamera(CameraRequest(left, top, w, h, facing == "front"))
        }

        @JavascriptInterface
        fun flipCamera() = onPage { camera?.flip { notifyCamera(false) } }

        @JavascriptInterface
        fun closeCamera() {
            handler.post { stopCamera() } // closing is always safe
        }

        private fun onPage(action: () -> Unit) {
            val gen = cameraGen
            handler.post {
                if (resumed && gen == cameraGen && webView?.url?.startsWith(allowedPrefix) == true) action()
            }
        }
    }

    private fun startCamera(request: CameraRequest) {
        if (!resumed) return
        if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            pendingCamera = request // a repeat open while asking only updates where / which way
            if (!awaitingCameraPermission) {
                awaitingCameraPermission = true
                requestPermissions(arrayOf(Manifest.permission.CAMERA), REQUEST_CAMERA)
            }
            return
        }
        val cam = camera ?: return
        cam.open(request.left, request.top, request.width, request.height, request.front) { notifyCamera(false) }
        notifyCamera(true)
    }

    private fun stopCamera() {
        cameraGen++
        pendingCamera = null
        camera?.close()
        notifyCamera(false)
    }

    /** Tells the page (src/lib/components/mobile/) whether the camera is open. */
    private fun notifyCamera(on: Boolean) {
        webView?.evaluateJavascript(
            "window.dispatchEvent(new CustomEvent('pa-camera',{detail:{on:$on}}))", null
        )
    }

    // ── Device page (FEAT-MP4) ──────────────────────────────────────────────

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

    private fun showDevices() {
        closeScanner()
        page = Page.DEVICES
        camera?.close()
        camera = null
        webView?.destroy()
        webView = null
        shownPa = null
        handler.removeCallbacksAndMessages(RETRY)

        val occupant = Config.occupant(this)
        val list = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(16), dp(24), dp(16), dp(16))
            addView(TextView(this@MainActivity).apply { text = "Pocket Agent · 电脑"; textSize = 20f })
        }
        val pas = Config.pas(this)
        if (pas.isEmpty()) {
            list.addView(TextView(this).apply {
                text = "还没有配对的电脑。在电脑上打开 Pocket Agent 设置 → 设备 → 添加手机，再点下面的「扫码添加电脑」。"
                setPadding(0, dp(16), 0, dp(16))
            })
        }
        for (pa in pas) list.addView(deviceRow(pa, occupant))
        list.addView(Button(this).apply { text = "扫码添加电脑"; setOnClickListener { showScanner() } })
        setContentView(ScrollView(this).apply { addView(list) })
    }

    private fun deviceRow(pa: Pa, occupant: String?): LinearLayout {
        val online = AudioService.isOnline(pa.key)
        val busyWith = occupant?.takeIf { it != pa.key }?.let { Config.pa(this, it)?.name }
        val (status, action, enabled) = when {
            occupant == pa.key -> Triple("正在使用" + if (online) "" else " · 离线", "打开", true)
            busyWith != null -> Triple("先让「$busyWith」回到电脑", "接入", false)
            !online -> Triple("离线", "接入", false)
            else -> Triple("在线", "接入", true)
        }
        return LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, dp(12), 0, dp(12))
            alpha = if (enabled) 1f else 0.5f
            addView(LinearLayout(this@MainActivity).apply {
                orientation = LinearLayout.VERTICAL
                addView(TextView(this@MainActivity).apply { text = pa.name; textSize = 17f })
                addView(TextView(this@MainActivity).apply { text = status; textSize = 13f })
            }, LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f))
            addView(Button(this@MainActivity).apply {
                text = action
                isEnabled = enabled
                setOnClickListener { enterPa(pa) }
            })
            setOnLongClickListener { confirmRemove(pa); true }
        }
    }

    private fun enterPa(pa: Pa) {
        if (Config.occupant(this) == pa.key) return route(pa.key)
        if (!AudioService.enter(pa.key)) {
            Toast.makeText(this, "现在不能接入「${pa.name}」", Toast.LENGTH_SHORT).show()
            showDevices()
        } // else onOccupant opens its page
    }

    private fun confirmRemove(pa: Pa) {
        AlertDialog.Builder(this)
            .setTitle("移除「${pa.name}」?")
            .setMessage("手机上不再保存这台电脑。电脑的设置 → 设备里也可以移除这台手机。")
            .setNegativeButton("取消", null)
            .setPositiveButton("移除") { _, _ ->
                if (Config.occupant(this) == pa.key) AudioService.leave()
                Config.remove(this, pa.key)
                AudioService.refresh()
                showDevices()
            }
            .show()
    }

    // ── Adding a computer by QR code (FEAT-QR2 / SEC-QR2) ───────────────────

    private fun showScanner() {
        if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            pendingScan = true
            if (!awaitingCameraPermission) {
                awaitingCameraPermission = true
                requestPermissions(arrayOf(Manifest.permission.CAMERA), REQUEST_CAMERA)
            }
            return
        }
        pendingScan = false
        if (!resumed || webView != null || scanner != null) return
        val root = FrameLayout(this).apply { setBackgroundColor(0xFF000000.toInt()) }
        root.addView(TextView(this).apply {
            text = "对准电脑设置 → 设备里的二维码"
            textSize = 16f
            setTextColor(0xFFFFFFFF.toInt())
            setShadowLayer(4f, 0f, 0f, 0xFF000000.toInt())
            setPadding(dp(16), dp(32), dp(16), dp(16))
        }, FrameLayout.LayoutParams(-1, -2, Gravity.TOP))
        root.addView(Button(this).apply { text = "取消"; setOnClickListener { showDevices() } },
            FrameLayout.LayoutParams(-2, -2, Gravity.BOTTOM or Gravity.CENTER_HORIZONTAL).apply { bottomMargin = dp(32) })
        scanner = QrScanner(this, root, onFound = ::scanned, onFailed = {
            toast("打不开相机")
            showDevices()
        })
        page = Page.SCANNER
        setContentView(root)
    }

    private fun closeScanner() {
        scanner?.close()
        scanner = null
    }

    private fun scanned(text: String) {
        if (scanner == null) return // left the page meanwhile
        closeScanner()
        val qr = PairQr.parse(text)
        if (qr == null) {
            toast("二维码无效，请在电脑上重新点「添加手机」")
            return showDevices()
        }
        page = Page.PAIRING
        setContentView(TextView(this).apply {
            this.text = "正在配对…"
            textSize = 18f
            gravity = Gravity.CENTER
        })
        val phoneId = Config.phoneId(this)
        Thread {
            val result = Pairing.pair(qr, phoneId, Config.phoneName())
            runOnUiThread {
                if (isDestroyed) return@runOnUiThread
                when (result) {
                    is PairResult.Paired -> return@runOnUiThread paired(
                        Config.add(this, result.paId, result.name.ifEmpty { result.host }, result.host, result.token),
                    )
                    PairResult.Expired -> toast("二维码已失效，请在电脑上重新点「添加手机」")
                    PairResult.Unreachable -> toast("连不上这台电脑：手机和电脑要连在同一个局域网")
                    PairResult.NotThatComputer -> toast("对方没能证明自己是显示二维码的那台电脑，没有保存。请重新扫码")
                    is PairResult.Failed -> toast("配对失败(${result.status})")
                }
                if (page == Page.PAIRING) showDevices() // else a PA took the phone meanwhile
            }
        }.start()
    }

    private fun paired(pa: Pa) {
        toast("已添加「${pa.name}」")
        AudioService.start(this) // first pairing starts it; later ones make it re-read the list
        if (page == Page.PAIRING) showDevices()
    }

    private fun toast(text: String) = Toast.makeText(this, text, Toast.LENGTH_LONG).show()

    companion object {
        private const val REQUEST_MIC = 1
        private const val REQUEST_CAMERA = 2
        private const val RETRY_MS = 3000L
        private const val VOICE_WINDOW_MS = 3 * 60 * 1000L
        private const val EXTRA_WAKE = "com.hermes.pocketagent.phone.WAKE"
        private val RETRY = Any()
        private val VOICE = Any()

        private fun wakeIntent(context: Context) = Intent(context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_REORDER_TO_FRONT)
            .putExtra(EXTRA_WAKE, true)

        /** The wake word with no activity alive: start one, it lights the screen itself. */
        fun wake(context: Context) = context.startActivity(wakeIntent(context))
    }
}
