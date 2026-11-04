package com.hermes.pocketagent.phone

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
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
import android.widget.TextView
import android.widget.Toast

/**
 * Full-screen WebView running PA's own frontend from the Mac (`/ui/`), plus a
 * one-time form for the Mac address and key. Back opens the form again.
 * Audio lives in [AudioService] so it keeps running with the screen off.
 */
class MainActivity : Activity() {
    private val handler = Handler(Looper.getMainLooper())
    private var webView: WebView? = null
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
    private val powerReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) = keepScreenOnWhenPlugged(intent)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        registerReceiver(powerReceiver, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
            ?.let { keepScreenOnWhenPlugged(it) }
        if (Config.load(this) == null) showSettings() else start()
        AudioService.onDisconnect = disconnect
    }

    override fun onResume() {
        super.onResume()
        resumed = true
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
        if (!awaitingCameraPermission) stopCamera()
    }

    override fun onStop() {
        super.onStop()
        stopCamera()
    }

    override fun onDestroy() {
        if (AudioService.onDisconnect === disconnect) AudioService.onDisconnect = null
        camera?.close()
        unregisterReceiver(powerReceiver)
        handler.removeCallbacksAndMessages(null)
        webView?.destroy()
        super.onDestroy()
    }

    @Deprecated("Back opens the connection form; predictive back is not opted in.")
    override fun onBackPressed() {
        if (webView != null) showSettings() else super.onBackPressed()
    }

    private fun keepScreenOnWhenPlugged(battery: Intent) {
        val plugged = battery.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) != 0
        if (plugged) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        else window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
    }

    private fun start() {
        val config = Config.load(this) ?: return showSettings()
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQUEST_MIC)
            return
        }
        // Restart so a changed address / key takes effect.
        stopService(Intent(this, AudioService::class.java))
        startForegroundService(Intent(this, AudioService::class.java))
        showWebView(config)
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
            Config.load(this)?.let { showWebView(it) }
        }
    }

    @SuppressLint("SetJavaScriptEnabled", "JavascriptInterface")
    private fun showWebView(config: Config) {
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

    private fun showSettings() {
        camera?.close()
        camera = null
        webView?.destroy()
        webView = null
        handler.removeCallbacksAndMessages(RETRY)
        val current = Config.load(this)
        val pad = (16 * resources.displayMetrics.density).toInt()
        val host = EditText(this).apply {
            hint = "Mac 地址，例如 192.168.1.20"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
            setText(current?.host.orEmpty())
        }
        val key = EditText(this).apply {
            hint = "API_SERVER_KEY"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            setText(current?.key.orEmpty())
        }
        val save = Button(this).apply {
            text = "连接"
            setOnClickListener {
                if (host.text.isBlank() || key.text.isBlank()) {
                    Toast.makeText(this@MainActivity, "地址和 key 都要填", Toast.LENGTH_SHORT).show()
                    return@setOnClickListener
                }
                Config.save(this@MainActivity, Config(host.text.toString(), key.text.toString()))
                start()
            }
        }
        setContentView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(pad, pad, pad, pad)
            addView(TextView(this@MainActivity).apply { text = "连接到 Mac 上的 Pocket Agent"; textSize = 18f })
            addView(host)
            addView(key)
            addView(save)
        })
    }

    private companion object {
        const val REQUEST_MIC = 1
        const val REQUEST_CAMERA = 2
        const val RETRY_MS = 3000L
        val RETRY = Any()
    }
}
