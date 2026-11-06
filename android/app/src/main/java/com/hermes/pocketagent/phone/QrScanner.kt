package com.hermes.pocketagent.phone

import android.content.Context
import android.util.Log
import android.widget.FrameLayout
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.ReaderException
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import java.util.concurrent.Executors

/**
 * Full-screen viewfinder that reads the PA pairing QR code (FEAT-QR2). The Y
 * plane of each analysis frame goes to ZXing; the first `pocketagent://pair`
 * text is handed to [onFound] once, on the main thread. Same self-owned
 * lifecycle as [PhoneCamera]. Frames are never stored or logged.
 */
class QrScanner(
    private val context: Context,
    root: FrameLayout,
    private val onFound: (String) -> Unit,
    private val onFailed: () -> Unit,
) : LifecycleOwner {
    private val registry = LifecycleRegistry(this)
    override fun getLifecycle(): Lifecycle = registry

    private val view = PreviewView(context).also {
        it.scaleType = PreviewView.ScaleType.FILL_CENTER
        root.addView(it, 0, FrameLayout.LayoutParams(-1, -1))
    }
    private val analyzer = Executors.newSingleThreadExecutor()
    private val reader = QRCodeReader()
    private val hints = mapOf(DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE))
    private var provider: ProcessCameraProvider? = null
    @Volatile private var done = false

    init {
        val future = ProcessCameraProvider.getInstance(context)
        future.addListener({
            if (done) return@addListener
            try {
                val cameraProvider = future.get()
                cameraProvider.unbindAll()
                val preview = Preview.Builder().build().also { it.setSurfaceProvider(view.surfaceProvider) }
                val analysis = ImageAnalysis.Builder()
                    .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                    .build()
                    .also { it.setAnalyzer(analyzer, ::analyze) }
                registry.currentState = Lifecycle.State.RESUMED
                cameraProvider.bindToLifecycle(this, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
                provider = cameraProvider
            } catch (e: Exception) {
                Log.w("PocketAgent", "scanner bind failed: ${e.message}")
                close()
                onFailed()
            }
        }, context.mainExecutor)
    }

    private fun analyze(image: ImageProxy) {
        image.use {
            if (done) return
            val plane = it.planes[0]
            val (w, h) = it.width to it.height
            val buffer = plane.buffer
            val y = ByteArray(w * h)
            for (row in 0 until h) {
                buffer.position(row * plane.rowStride)
                buffer.get(y, row * w, w)
            }
            val text = try {
                reader.decode(BinaryBitmap(HybridBinarizer(PlanarYUVLuminanceSource(y, w, h, 0, 0, w, h, false))), hints).text
            } catch (e: ReaderException) {
                null
            } finally {
                reader.reset()
            }
            if (text != null && text.startsWith("pocketagent://pair")) {
                done = true
                context.mainExecutor.execute { onFound(text) }
            }
        }
    }

    fun close() {
        done = true
        provider?.unbindAll()
        provider = null
        (view.parent as? FrameLayout)?.removeView(view)
        analyzer.shutdown()
        if (registry.currentState.isAtLeast(Lifecycle.State.CREATED)) registry.currentState = Lifecycle.State.CREATED
    }
}
