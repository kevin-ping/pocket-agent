package com.hermes.pocketagent.phone

import android.animation.ValueAnimator
import android.content.Context
import android.graphics.Canvas
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.graphics.Shader
import android.view.View

/**
 * The scanner page's mask over the camera (FEAT-SC1): #0E0E1A at 60% with a
 * clear rounded viewfinder, accent L-shaped corners and a scan line that
 * sweeps up and down every 2 s. With system animations off the line is not
 * drawn. [confirm] outlines the whole viewfinder once a code is read.
 */
class ScanOverlay(context: Context) : View(context) {
    private val density = resources.displayMetrics.density
    private fun dp(v: Float) = v * density

    /** The viewfinder, in this view's coordinates; set once laid out. */
    val window = RectF()
    var onWindow: ((RectF) -> Unit)? = null

    private val radius = dp(20f)
    private val mask = Path().apply { fillType = Path.FillType.EVEN_ODD }
    private val maskPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = MASK }
    private val corners = Path()
    private val cornerPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = ACCENT
        style = Paint.Style.STROKE
        strokeWidth = dp(4f)
        strokeCap = Paint.Cap.ROUND
    }
    private val linePaint = Paint(Paint.ANTI_ALIAS_FLAG)
    private var linePos = 0f
    private var confirmed = false
    // False when ANIMATOR_DURATION_SCALE is 0 ("Remove animations"), as the system actually applies it.
    private val animate = ValueAnimator.areAnimatorsEnabled()
    private val sweep = ValueAnimator.ofFloat(0f, 1f).apply {
        duration = 2000
        repeatMode = ValueAnimator.REVERSE
        repeatCount = ValueAnimator.INFINITE
        addUpdateListener { linePos = it.animatedValue as Float; invalidate() }
    }

    init {
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_NO
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        val side = minOf(w * 0.68f, dp(280f))
        val left = (w - side) / 2
        val top = h * 0.42f - side / 2 // a little above the middle
        window.set(left, top, left + side, top + side)

        mask.reset()
        mask.addRect(0f, 0f, w.toFloat(), h.toFloat(), Path.Direction.CW)
        mask.addRoundRect(window, radius, radius, Path.Direction.CW)

        val len = dp(28f)
        val r = radius
        corners.reset()
        with(window) {
            corners.moveTo(left, top + len); corners.lineTo(left, top + r)
            corners.arcTo(left, top, left + 2 * r, top + 2 * r, 180f, 90f, false); corners.lineTo(left + len, top)
            corners.moveTo(right - len, top); corners.lineTo(right - r, top)
            corners.arcTo(right - 2 * r, top, right, top + 2 * r, 270f, 90f, false); corners.lineTo(right, top + len)
            corners.moveTo(right, bottom - len); corners.lineTo(right, bottom - r)
            corners.arcTo(right - 2 * r, bottom - 2 * r, right, bottom, 0f, 90f, false); corners.lineTo(right - len, bottom)
            corners.moveTo(left + len, bottom); corners.lineTo(left + r, bottom)
            corners.arcTo(left, bottom - 2 * r, left + 2 * r, bottom, 90f, 90f, false); corners.lineTo(left, bottom - len)
        }
        linePaint.shader = LinearGradient(
            window.left + dp(12f), 0f, window.right - dp(12f), 0f,
            intArrayOf(ACCENT and 0x00FFFFFF, ACCENT, ACCENT and 0x00FFFFFF), null, Shader.TileMode.CLAMP,
        )
        onWindow?.invoke(window)
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        if (animate && !confirmed) sweep.start()
    }

    override fun onDetachedFromWindow() {
        sweep.cancel()
        super.onDetachedFromWindow()
    }

    /** A code was read: stop the line and outline the whole viewfinder. */
    fun confirm() {
        confirmed = true
        sweep.cancel()
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        canvas.drawPath(mask, maskPaint)
        if (confirmed) {
            canvas.drawRoundRect(window, radius, radius, cornerPaint)
            return
        }
        canvas.drawPath(corners, cornerPaint)
        if (animate) {
            val inset = dp(16f)
            val y = window.top + inset + (window.height() - 2 * inset) * linePos
            canvas.drawRect(window.left + dp(12f), y - dp(1f), window.right - dp(12f), y + dp(1f), linePaint)
        }
    }

    companion object {
        const val BACKGROUND = 0xFF0E0E1A.toInt()
        const val MASK = 0x990E0E1A.toInt() // 60%
        const val ACCENT = 0xFFA0A8FF.toInt()
    }
}
