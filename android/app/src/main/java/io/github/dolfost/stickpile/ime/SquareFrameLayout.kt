package io.github.dolfost.stickpile.ime

import android.content.Context
import android.util.AttributeSet
import android.widget.FrameLayout

/** A cell of the picker's grid: as tall as it is wide. */
class SquareFrameLayout @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : FrameLayout(context, attrs) {
    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        super.onMeasure(widthMeasureSpec, widthMeasureSpec)
    }
}
