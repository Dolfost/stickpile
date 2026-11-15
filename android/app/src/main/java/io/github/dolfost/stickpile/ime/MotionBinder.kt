package io.github.dolfost.stickpile.ime

import android.content.Context
import android.graphics.drawable.Animatable
import android.widget.ImageView
import androidx.core.view.isVisible
import io.github.dolfost.stickpile.display.Autoplay
import io.github.dolfost.stickpile.motion.LoopingVideoView
import io.github.dolfost.stickpile.motion.MotionPolicy
import io.github.dolfost.stickpile.motion.Motions
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.MotionKind
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/**
 * Makes the picker's animations and videos move, in the grid and the
 * details ([Motions]): once the still is shown ([ThumbnailLoader]), an
 * animated picture takes its place in the same [ImageView], or a video
 * plays in a [LoopingVideoView] over it. As [autoplay] lets it.
 */
internal class MotionBinder(
    private val scope: CoroutineScope,
    private val library: ImeLibrary,
    private val context: Context,
) {
    var autoplay: Autoplay = Autoplay.ALWAYS

    /**
     * Moves [item] in [image] (which [ThumbnailLoader.bind] gave its still,
     * in [thumbnail] when not at once) or [video], decoded at most [maxPx];
     * the job to cancel when the views are reused, if any.
     */
    fun bind(item: Item, image: ImageView, video: LoopingVideoView, maxPx: Int, thumbnail: Job?): Job? {
        unbind(video)
        val source = MotionPolicy.source(item.kind, autoplay, Motions.metered(context)) ?: return null
        return scope.launch {
            thumbnail?.join()
            val motion = Motions.motion(item.id, source, library::motion) ?: return@launch
            if (image.tag != item.id) return@launch
            when (motion.kind) {
                MotionKind.PICTURE -> {
                    val picture = Motions.picture(item.id, motion, maxPx) ?: return@launch
                    if (image.tag != item.id) return@launch
                    // Shown elsewhere since (the details): set again, it draws here once more.
                    if (image.drawable === picture) image.setImageDrawable(null)
                    image.setImageDrawable(picture)
                    (picture as? Animatable)?.start()
                }
                MotionKind.VIDEO -> {
                    video.listener = object : LoopingVideoView.Listener {
                        override fun failed() {
                            Motions.failed(item.id)
                            video.isVisible = false
                        }
                    }
                    video.isVisible = true
                    video.play(motion.path)
                }
            }
        }
    }

    /** [video] plays nothing (its view is reused, or the details closed). */
    fun unbind(video: LoopingVideoView) {
        video.listener = null
        video.play(null)
        video.isVisible = false
    }
}
