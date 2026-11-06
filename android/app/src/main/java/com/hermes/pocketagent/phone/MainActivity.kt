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
import android.text.InputType
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
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * Two screens. The device page (native) lists the paired computers: who uses
 * the phone, who is online, 「接入」, and adding one (LAN scan → pairing code,
 * or a typed address). While a PA uses the phone (the occupant, decided in
 * [AudioService]) a full-screen WebView runs that PA's own frontend (`/ui/`);
 * Back returns to the device page. Audio lives in [AudioService] so it keeps
 * running with the screen off.
 */
class MainActivity : Activity() {
    private val handler = Handler(Looper.getMainLooper())
    private var webView: WebView? = null
    private var shownPa: String? = null // the PA whose page the WebView shows
    private var discovery: Discovery? = null
    private var found: List<FoundPa> = emptyList()
    private var onFound: (() -> Unit)? = null // the open 「添加电脑」 list
    private val http = OkHttpClient.Builder().callTimeout(10, TimeUnit.SECONDS).build()
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
    private val devicesChanged: () -> Unit = { if (webView == null) showDevices() }

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
        if (webView == null) discovery?.start()
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
        discovery?.stop()
        if (!awaitingCameraPermission) stopCamera()
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
        discovery?.stop()
        camera?.close()
        unregisterReceiver(powerReceiver)
        handler.removeCallbacksAndMessages(null)
        webView?.destroy()
        super.onDestroy()
    }

    @Deprecated("Back opens the device page; predictive back is not opted in.")
    override fun onBackPressed() {
        if (webView != null) showDevices() else super.onBackPressed()
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
        if (pa == null) showDevices() else if (webView == null || shownPa != pa.key) showWebView(pa)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, results: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, results)
        if (requestCode == REQUEST_CAMERA) {
            awaitingCameraPermission = false
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
        discovery?.stop()
        discovery = null
        onFound = null
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
        camera?.close()
        camera = null
        webView?.destroy()
        webView = null
        shownPa = null
        handler.removeCallbacksAndMessages(RETRY)
        if (discovery == null) {
            discovery = Discovery(this) { list ->
                val before = Config.pas(this).map { movedTo(it) }
                found = list
                onFound?.invoke()
                if (webView == null && Config.pas(this).map { movedTo(it) } != before) showDevices()
            }
        }
        if (resumed) discovery?.start()

        val occupant = Config.occupant(this)
        val list = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(16), dp(24), dp(16), dp(16))
            addView(TextView(this@MainActivity).apply { text = "Pocket Agent · 电脑"; textSize = 20f })
        }
        val pas = Config.pas(this)
        if (pas.isEmpty()) {
            list.addView(TextView(this).apply {
                text = "还没有配对的电脑。点下面的「添加电脑」,或者手动输入地址。"
                setPadding(0, dp(16), 0, dp(16))
            })
        }
        for (pa in pas) list.addView(deviceRow(pa, occupant))
        list.addView(Button(this).apply { text = "添加电脑"; setOnClickListener { showAddComputer() } })
        list.addView(Button(this).apply { text = "手动输入地址"; setOnClickListener { showManualForm() } })
        setContentView(ScrollView(this).apply { addView(list) })
    }

    private fun deviceRow(pa: Pa, occupant: String?): LinearLayout {
        val online = AudioService.isOnline(pa.key)
        val busyWith = occupant?.takeIf { it != pa.key }?.let { Config.pa(this, it)?.name }
        val moved = if (online) null else movedTo(pa)
        val (status, action, enabled) = when {
            moved != null -> Triple("地址变了:$moved", "重新配对", true)
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
                setOnClickListener { if (moved != null) confirmRepair(pa, moved) else enterPa(pa) }
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

    /**
     * The new address a paired PA announces (DHCP gave its computer another IP),
     * or null. mDNS is unauthenticated — anyone on the LAN can announce a paired
     * PA's id — so the phone never sends the saved token there by itself.
     */
    private fun movedTo(pa: Pa): String? =
        found.firstOrNull { pa.paId.isNotEmpty() && it.paId == pa.paId }?.host
            ?.takeIf { Pa.hostPort(it) != Pa.hostPort(pa.host) }

    /** Pairing again at the new address: only the real computer's screen shows the code. */
    private fun confirmRepair(pa: Pa, host: String) {
        AlertDialog.Builder(this)
            .setTitle("「${pa.name}」换了地址？")
            .setMessage("局域网里有一台自称「${pa.name}」的电脑在 $host。为了安全，要在那台电脑的屏幕上看配对码，重新配对一次。")
            .setNegativeButton("取消", null)
            .setPositiveButton("重新配对") { _, _ -> startPairing(host, pa.name) }
            .show()
    }

    private fun showAddComputer() {
        val box = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(dp(16), dp(8), dp(16), 0) }
        val dialog = AlertDialog.Builder(this)
            .setTitle("添加电脑")
            .setView(ScrollView(this).apply { addView(box) })
            .setNegativeButton("取消", null)
            .setOnDismissListener { onFound = null }
            .create()
        fun render() {
            box.removeAllViews()
            val paired = Config.pas(this).map { it.paId }.filter { it.isNotEmpty() }.toSet()
            val candidates = found.filter { it.paId !in paired }
            if (candidates.isEmpty()) {
                box.addView(TextView(this).apply {
                    text = "正在局域网里找运行 Pocket Agent 的电脑…\n找不到时用「手动输入地址」。"
                })
            }
            for (pa in candidates) box.addView(Button(this).apply {
                text = "${pa.name}  (${pa.host})"
                setOnClickListener { dialog.dismiss(); startPairing(pa.host, pa.name) }
            })
        }
        onFound = { render() }
        render()
        dialog.show()
    }

    private fun showManualForm() {
        val host = EditText(this).apply {
            hint = "电脑地址，例如 192.168.1.20"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
        }
        val key = EditText(this).apply {
            hint = "API_SERVER_KEY(留空则用配对码)"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
        }
        AlertDialog.Builder(this)
            .setTitle("手动输入地址")
            .setView(LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(dp(16), dp(8), dp(16), 0)
                addView(host)
                addView(key)
            })
            .setNegativeButton("取消", null)
            .setPositiveButton("连接") { _, _ ->
                val typed = host.text.toString().trim()
                val h = Pa.parseHost(typed)
                val k = key.text.toString().trim()
                when {
                    typed.isEmpty() -> Toast.makeText(this, "要填电脑地址", Toast.LENGTH_SHORT).show()
                    h == null -> toast("地址不对:$typed(例如 192.168.1.20 或 192.168.1.20:8650)")
                    k.isEmpty() -> startPairing(h, h)
                    else -> paired(Config.add(this, "", h, h, k))
                }
            }
            .show()
    }

    private fun post(host: String, path: String, body: JSONObject, done: (Int, JSONObject?) -> Unit) {
        val request = try {
            Request.Builder()
                .url("http://${Pa.hostPort(host)}$path")
                .post(body.toString().toRequestBody("application/json".toMediaType()))
                .build()
        } catch (e: IllegalArgumentException) {
            return toast("地址不对:$host")
        }
        Thread {
            val result = try {
                http.newCall(request).execute().use { r ->
                    r.code to (try { JSONObject(r.body?.string().orEmpty()) } catch (e: Exception) { null })
                }
            } catch (e: Exception) {
                0 to null
            }
            runOnUiThread { done(result.first, result.second) }
        }.start()
    }

    /** /pair/start: the computer shows a code; the user types it here. */
    private fun startPairing(host: String, name: String) {
        val body = JSONObject().put("phone_id", Config.phoneId(this)).put("phone_name", Config.phoneName())
        post(host, "/pair/start", body) { code, reply ->
            when (code) {
                200 -> askCode(host, reply?.optString("pa_name").orEmpty().ifEmpty { name })
                409 -> toast("另一台手机正在配对这台电脑，稍后再试")
                429 -> toast(if (reply?.optString("error").orEmpty().contains("5 minutes")) "配对码错误次数太多，5 分钟后再试" else "请稍等几秒再试")
                0 -> toast("连不上 $host")
                else -> toast("配对失败($code)")
            }
        }
    }

    private fun askCode(host: String, name: String, hint: String? = null) {
        val input = EditText(this).apply {
            this.hint = "6 位配对码"
            inputType = InputType.TYPE_CLASS_NUMBER
        }
        AlertDialog.Builder(this)
            .setTitle("配对「$name」")
            .setMessage(hint ?: "在「$name」的屏幕上会显示配对码，2 分钟内有效。")
            .setView(LinearLayout(this).apply { setPadding(dp(16), 0, dp(16), 0); addView(input, LinearLayout.LayoutParams(-1, -2)) })
            .setNegativeButton("取消", null)
            .setPositiveButton("配对") { _, _ -> confirmPairing(host, name, input.text.toString().trim()) }
            .show()
    }

    private fun confirmPairing(host: String, name: String, code: String) {
        val body = JSONObject().put("phone_id", Config.phoneId(this)).put("code", code)
        post(host, "/pair/confirm", body) { status, reply ->
            when {
                status == 200 && reply != null -> paired(Config.add(
                    this, reply.optString("pa_id"), reply.optString("pa_name").ifEmpty { name }, host, reply.getString("token"),
                ))
                status == 403 -> {
                    val left = reply?.optInt("attempts_left", 0) ?: 0
                    if (left > 0) askCode(host, name, "配对码不对，还可以再试 $left 次。")
                    else toast("错误次数太多，这次配对已作废，5 分钟后再重新添加")
                }
                status == 410 -> toast("配对码已失效，请重新添加")
                status == 0 -> toast("连不上 $host")
                else -> toast("配对失败($status)")
            }
        }
    }

    private fun paired(pa: Pa) {
        toast("已添加「${pa.name}」")
        AudioService.start(this) // first pairing starts it; later ones make it re-read the list
        showDevices()
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
