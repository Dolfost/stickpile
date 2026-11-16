package io.github.dolfost.stickpile.ime

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.util.AttributeSet
import androidx.appcompat.widget.AppCompatTextView
import androidx.core.content.ContextCompat
import androidx.core.graphics.withTranslation
import io.github.dolfost.stickpile.R

/**
 * Text with a dark outline around the letters, drawn under them: the
 * white lines of a tile read on any sticker without a band behind them.
 * The outline is the text again, stroked [R.dimen.ime_tile_halo] wide in
 * [R.color.ime_tile_halo]; pictures in the text (the stats line's icons)
 * carry their own.
 */
class HaloTextView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : AppCompatTextView(context, attrs) {

    private val haloWidth = resources.getDimension(R.dimen.ime_tile_halo)
    private val haloColor = ContextCompat.getColor(context, R.color.ime_tile_halo)

    override fun onDraw(canvas: Canvas) {
        val layout = layout
        if (layout != null) {
            val paint = paint
            val color = paint.color
            val style = paint.style
            val width = paint.strokeWidth
            val join = paint.strokeJoin
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = haloWidth
            paint.strokeJoin = Paint.Join.ROUND
            paint.color = haloColor
            canvas.withTranslation(compoundPaddingLeft.toFloat(), extendedPaddingTop.toFloat()) {
                layout.draw(this)
            }
            paint.style = style
            paint.strokeWidth = width
            paint.strokeJoin = join
            paint.color = color
        }
        super.onDraw(canvas)
    }
}
