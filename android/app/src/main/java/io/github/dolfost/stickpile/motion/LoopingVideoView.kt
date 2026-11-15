package io.github.dolfost.stickpile.motion

import android.content.Context
import android.graphics.Matrix
import android.graphics.SurfaceTexture
import android.media.MediaPlayer
import android.util.AttributeSet
import android.util.Log
import android.view.Surface
import android.view.TextureView
import android.view.View
import java.io.FileInputStream

/**
 * A video playing over and over without sound, fitted in the view like a
 * picture ([VideoFit]): a tile's or the details' moving item. It plays only
 * while it is on screen, and at most [MAX_PLAYING] play in the process at
 * once ([Slots]): the others wait, showing whatever is under them. The view
 * stays clear until the first frame is drawn, so the still under it shows
 * meanwhile, and again once it stops.
 *
 * Main thread only. [play] the file (null: none); a file that will not
 * play is reported to [listener] once and not tried again.
 */
class LoopingVideoView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : TextureView(context, attrs), TextureView.SurfaceTextureListener {

    interface Listener {
        /** The first frame is drawn: the video covers what is under it. */
        fun started() {}

        /** The file did not play; the view stays clear. */
        fun failed()
    }

    var listener: Listener? = null

    private var path: String? = null
    private var player: MediaPlayer? = null
    private var surface: Surface? = null
    private var videoWidth = 0
    private var videoHeight = 0

    init {
        isOpaque = false
        surfaceTextureListener = this
        alpha = 0f
    }

    /** Play [path] (an absolute file), or nothing for null. The same file again changes nothing. */
    fun play(path: String?) {
        if (path == this.path) return
        stopPlaying()
        this.path = path
        videoWidth = 0
        videoHeight = 0
        start()
    }

    /** On screen, with a surface to draw on and a file to play. */
    private fun canPlay(): Boolean =
        path != null && surfaceTexture != null && isAttachedToWindow && isShown && windowVisibility == View.VISIBLE

    private fun start() {
        if (player != null) return
        if (!canPlay()) {
            // A slot handed over to a view that cannot use it now goes on.
            if (slots.holds(this)) slots.give(this)?.start()
            return
        }
        if (!slots.take(this)) return
        val file = path ?: return
        val texture = surfaceTexture ?: return
        val s = Surface(texture)
        val mp = MediaPlayer()
        surface = s
        player = mp
        try {
            FileInputStream(file).use { mp.setDataSource(it.fd) }
            mp.setSurface(s)
            mp.isLooping = true
            mp.setVolume(0f, 0f)
            mp.setOnPreparedListener { if (player === it) it.start() }
            mp.setOnVideoSizeChangedListener { p, w, h ->
                if (player === p) {
                    videoWidth = w
                    videoHeight = h
                    fit()
                }
            }
            mp.setOnInfoListener { p, what, _ ->
                if (player === p && what == MediaPlayer.MEDIA_INFO_VIDEO_RENDERING_START) {
                    alpha = 1f
                    listener?.started()
                }
                false
            }
            mp.setOnErrorListener { p, what, extra ->
                if (player === p) fail("error $what/$extra")
                true
            }
            mp.prepareAsync()
        } catch (e: Exception) {
            // IOException, IllegalStateException, SecurityException: a file that does not play.
            fail(e.javaClass.simpleName)
        }
    }

    private fun fail(why: String) {
        Log.d(TAG, "video not played: $why")
        stopPlaying()
        path = null
        listener?.failed()
    }

    private fun stopPlaying() {
        val mp = player
        player = null
        mp?.release()
        surface?.release()
        surface = null
        alpha = 0f
        slots.give(this)?.start()
    }

    private fun fit() {
        val (sx, sy) = VideoFit.scale(width, height, videoWidth, videoHeight) ?: return
        setTransform(Matrix().apply { setScale(sx, sy, width / 2f, height / 2f) })
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        fit()
    }

    override fun onVisibilityAggregated(isVisible: Boolean) {
        super.onVisibilityAggregated(isVisible)
        if (isVisible) start() else stopPlaying()
    }

    override fun onDetachedFromWindow() {
        stopPlaying()
        super.onDetachedFromWindow()
    }

    override fun onSurfaceTextureAvailable(texture: SurfaceTexture, width: Int, height: Int) {
        start()
    }

    override fun onSurfaceTextureSizeChanged(texture: SurfaceTexture, width: Int, height: Int) {
        fit()
    }

    override fun onSurfaceTextureDestroyed(texture: SurfaceTexture): Boolean {
        stopPlaying()
        return true
    }

    override fun onSurfaceTextureUpdated(texture: SurfaceTexture) = Unit

    companion object {
        private const val TAG = "stickpile.motion"

        /** Videos playing at once in the process: each holds a decoder, of which a phone has few. */
        const val MAX_PLAYING = 6

        private val slots = Slots<LoopingVideoView>(MAX_PLAYING)
    }
}
