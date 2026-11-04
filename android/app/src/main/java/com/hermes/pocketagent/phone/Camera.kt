package com.hermes.pocketagent.phone

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.os.Handler
import android.os.Looper
import android.util.Base64
import android.util.Log
import android.widget.FrameLayout
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageCapture
import androidx.camera.core.ImageCaptureException
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import java.io.ByteArrayOutputStream
import java.util.concurrent.Executors

/**
 * Frames for Hermes' `phone_camera_snapshot` (FEAT-CAM1). [PhoneCamera] attaches
 * its ImageCapture while the user has the camera open and the app is in the
 * foreground; [AudioService] asks for one frame per PA request and reports
 * open / closed. Frames stay in memory: never written to disk, never logged.
 */
object CameraHub {
    private const val MAX_EDGE = 1280
    private const val QUALITY = 80

    private val main = Handler(Looper.getMainLooper())
    private val encoder = Executors.newSingleThreadExecutor()
    private var capture: ImageCapture? = null // main thread only
    @Volatile private var on = false
    @Volatile private var front = false
    @Volatile private var switching = false // rebinding to another lens: no frames until attach

    /** Called with [stateJson] whenever it changes (any thread). Set by AudioService. */
    @Volatile var listener: ((String) -> Unit)? = null

    /** The `/audio` camera report: the lens actually bound, and whether it is still switching. */
    fun stateJson(): String =
        if (on) """{"type":"camera","on":true,"facing":"${if (front) "front" else "back"}","switching":$switching}"""
        else """{"type":"camera","on":false}"""

    fun attach(imageCapture: ImageCapture, useFront: Boolean) {
        capture = imageCapture
        on = true
        front = useFront
        switching = false
        listener?.invoke(stateJson())
    }

    /** The open camera starts rebinding to `useFront`: no frame from the old lens meanwhile. */
    fun switchTo(useFront: Boolean) {
        if (!on) return
        capture = null
        front = useFront
        switching = true
        listener?.invoke(stateJson())
    }

    fun detach() {
        if (!on) return
        capture = null
        on = false
        switching = false
        listener?.invoke(stateJson())
    }

    /** One frame as base64 JPEG (long edge ≤ 1280), or null when the camera is closed or fails. */
    fun snapshot(done: (String?) -> Unit) = main.post {
        val imageCapture = capture ?: return@post done(null)
        imageCapture.takePicture(encoder, object : ImageCapture.OnImageCapturedCallback() {
            override fun onCaptureSuccess(image: ImageProxy) {
                done(try { encode(image) } catch (e: Exception) { Log.w("PocketAgent", "snapshot encode failed: ${e.message}"); null })
            }

            override fun onError(e: ImageCaptureException) {
                Log.w("PocketAgent", "snapshot failed: ${e.message}")
                done(null)
            }
        })
    }

    private fun encode(image: ImageProxy): String {
        val rotation = image.imageInfo.rotationDegrees
        val jpeg = image.use {
            val buffer = it.planes[0].buffer
            ByteArray(buffer.remaining()).also { bytes -> buffer.get(bytes) }
        }
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(jpeg, 0, jpeg.size, bounds)
        var sample = 1
        while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= MAX_EDGE) sample *= 2
        val src = BitmapFactory.decodeByteArray(jpeg, 0, jpeg.size, BitmapFactory.Options().apply { inSampleSize = sample })
        val scale = minOf(1f, MAX_EDGE.toFloat() / maxOf(src.width, src.height))
        val matrix = Matrix().apply { postScale(scale, scale); postRotate(rotation.toFloat()) }
        val out = Bitmap.createBitmap(src, 0, 0, src.width, src.height, matrix, true)
        val bytes = ByteArrayOutputStream().use { stream ->
            out.compress(Bitmap.CompressFormat.JPEG, QUALITY, stream)
            stream.toByteArray()
        }
        if (out !== src) out.recycle()
        src.recycle()
        return Base64.encodeToString(bytes, Base64.NO_WRAP)
    }
}

/**
 * The viewfinder: a PreviewView laid over the WebView at the rectangle the page
 * reserved for it. Owns its own lifecycle so the plain Activity can stay as is.
 */
class PhoneCamera(private val context: Context, private val root: FrameLayout) : LifecycleOwner {
    private val registry = LifecycleRegistry(this)
    override fun getLifecycle(): Lifecycle = registry

    private var view: PreviewView? = null
    private var front = false
    private var provider: ProcessCameraProvider? = null
    private var generation = 0 // bumped by close() and each bind(); a bind finishing after that is dropped

    val isOpen get() = view != null

    /** Opens, or moves / switches the open camera. Rect in screen pixels. */
    fun open(left: Int, top: Int, width: Int, height: Int, useFront: Boolean, onFailed: () -> Unit) {
        val preview = view ?: PreviewView(context).also {
            it.scaleType = PreviewView.ScaleType.FILL_CENTER
            root.addView(it)
            view = it
        }
        preview.layoutParams = FrameLayout.LayoutParams(width, height).apply { leftMargin = left; topMargin = top }
        val rebind = provider == null || useFront != front
        front = useFront
        if (rebind) bind(onFailed)
    }

    fun flip(onFailed: () -> Unit) {
        if (!isOpen) return
        front = !front
        bind(onFailed)
    }

    fun close() {
        generation++
        provider?.unbindAll()
        provider = null
        CameraHub.detach()
        view?.let { root.removeView(it) }
        view = null
        if (registry.currentState.isAtLeast(Lifecycle.State.CREATED)) registry.currentState = Lifecycle.State.CREATED
    }

    private fun bind(onFailed: () -> Unit) {
        val gen = ++generation
        CameraHub.switchTo(front)
        val future = ProcessCameraProvider.getInstance(context)
        future.addListener({
            if (gen != generation) return@addListener // closed or rebound meanwhile
            val preview = view ?: return@addListener
            try {
                val cameraProvider = future.get()
                cameraProvider.unbindAll()
                val usecase = Preview.Builder().build().also { it.setSurfaceProvider(preview.surfaceProvider) }
                val imageCapture = ImageCapture.Builder()
                    .setCaptureMode(ImageCapture.CAPTURE_MODE_MINIMIZE_LATENCY)
                    .build()
                val selector = if (front) CameraSelector.DEFAULT_FRONT_CAMERA else CameraSelector.DEFAULT_BACK_CAMERA
                registry.currentState = Lifecycle.State.RESUMED
                cameraProvider.bindToLifecycle(this, selector, usecase, imageCapture)
                provider = cameraProvider
                CameraHub.attach(imageCapture, front)
            } catch (e: Exception) {
                Log.w("PocketAgent", "camera bind failed: ${e.message}")
                close()
                onFailed()
            }
        }, context.mainExecutor)
    }
}
