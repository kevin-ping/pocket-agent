package com.hermes.pocketagent.phone

import android.content.Context
import android.graphics.Rect
import android.graphics.RectF
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
 * text is handed to [onFound] once, on the main thread. Once [setViewfinder]
 * is called only codes inside the viewfinder count, not anywhere in the frame. Same self-owned
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
    private class Viewfinder(val box: RectF, val width: Float, val height: Float)
    @Volatile private var viewfinder: Viewfinder? = null

    /** [box] in the preview's pixels (the overlay covers the same area). */
    fun setViewfinder(box: RectF) {
        viewfinder = Viewfinder(RectF(box), view.width.toFloat(), view.height.toFloat())
    }

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
            val crop = crop(w, h, it.imageInfo.rotationDegrees)
            if (crop.width() <= 0 || crop.height() <= 0) return
            val text = try {
                val source = PlanarYUVLuminanceSource(y, w, h, crop.left, crop.top, crop.width(), crop.height(), false)
                reader.decode(BinaryBitmap(HybridBinarizer(source)), hints).text
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

    /**
     * The viewfinder (plus [MARGIN]) in the analysis image's own (sensor)
     * coordinates. The preview fills the view (FILL_CENTER) with the image
     * turned upright by [rotation]; both streams are 4:3.
     */
    private fun crop(w: Int, h: Int, rotation: Int): Rect {
        val finder = viewfinder ?: return Rect(0, 0, w, h)
        if (finder.width <= 0 || finder.height <= 0) return Rect(0, 0, w, h)
        val turned = rotation % 180 != 0
        val uw = (if (turned) h else w).toFloat() // upright size
        val uh = (if (turned) w else h).toFloat()
        val scale = maxOf(finder.width / uw, finder.height / uh)
        val dx = (finder.width - uw * scale) / 2
        val dy = (finder.height - uh * scale) / 2
        val m = finder.box.width() * MARGIN
        val l = ((finder.box.left - m - dx) / scale).coerceIn(0f, uw)
        val t = ((finder.box.top - m - dy) / scale).coerceIn(0f, uh)
        val r = ((finder.box.right + m - dx) / scale).coerceIn(0f, uw)
        val b = ((finder.box.bottom + m - dy) / scale).coerceIn(0f, uh)
        val sensor = when (rotation) {
            90 -> RectF(t, uw - r, b, uw - l)
            180 -> RectF(uw - r, uh - b, uw - l, uh - t)
            270 -> RectF(uh - b, l, uh - t, r)
            else -> RectF(l, t, r, b)
        }
        return Rect(sensor.left.toInt(), sensor.top.toInt(), sensor.right.toInt(), sensor.bottom.toInt())
            .apply { intersect(0, 0, w, h) }
    }

    fun close() {
        done = true
        provider?.unbindAll()
        provider = null
        (view.parent as? FrameLayout)?.removeView(view)
        analyzer.shutdown()
        if (registry.currentState.isAtLeast(Lifecycle.State.CREATED)) registry.currentState = Lifecycle.State.CREATED
    }

    private companion object {
        const val MARGIN = 0.1f // a code slightly past the corners still counts
    }
}
