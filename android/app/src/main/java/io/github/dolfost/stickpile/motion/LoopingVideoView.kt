package io.github.dolfost.stickpile.motion

import android.content.Context
import android.graphics.Matrix
import android.graphics.SurfaceTexture
import android.media.MediaPlayer
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
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
 *
 * The players are made, started and released on a thread of their own
 * ([Player]): each of those is a call to the media server, and a grid
 * that scrolls starts and stops several at once, which on the main thread
 * held up its frames.
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
    private var player: Player? = null
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
        player = Player(this, file, texture)
    }

    private fun fail(why: String) {
        Log.d(TAG, "video not played: $why")
        stopPlaying()
        path = null
        listener?.failed()
    }

    private fun stopPlaying() {
        val p = player
        player = null
        p?.release()
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

    /**
     * [texture] is released on the player thread (false: not by the view),
     * after the players that drew into it: that thread runs in order.
     */
    override fun onSurfaceTextureDestroyed(texture: SurfaceTexture): Boolean {
        stopPlaying()
        worker.post { texture.release() }
        return false
    }

    override fun onSurfaceTextureUpdated(texture: SurfaceTexture) = Unit

    /**
     * One [MediaPlayer] for [file] into [texture], living on the player
     * thread: it is made there, and its callbacks come there and go on to
     * the main thread, where they count only while [view] still plays it.
     */
    private class Player(private val view: LoopingVideoView, file: String, texture: SurfaceTexture) {
        // Only on the player thread.
        private var mp: MediaPlayer? = null
        private var surface: Surface? = null

        @Volatile
        private var released = false

        init {
            worker.post { open(file, texture) }
        }

        /** Stop and release; nothing more reaches the view. */
        fun release() {
            released = true
            worker.post {
                mp?.release()
                mp = null
                surface?.release()
                surface = null
            }
        }

        private fun open(file: String, texture: SurfaceTexture) {
            if (released) return
            val player = MediaPlayer()
            mp = player
            try {
                val s = Surface(texture)
                surface = s
                FileInputStream(file).use { player.setDataSource(it.fd) }
                player.setSurface(s)
                player.isLooping = true
                player.setVolume(0f, 0f)
                player.setOnPreparedListener { if (!released) it.start() }
                player.setOnVideoSizeChangedListener { _, w, h ->
                    onView {
                        videoWidth = w
                        videoHeight = h
                        fit()
                    }
                }
                player.setOnInfoListener { _, what, _ ->
                    if (what == MediaPlayer.MEDIA_INFO_VIDEO_RENDERING_START) {
                        onView {
                            alpha = 1f
                            listener?.started()
                        }
                    }
                    false
                }
                player.setOnErrorListener { _, what, extra ->
                    onView { fail("error $what/$extra") }
                    true
                }
                player.prepareAsync()
            } catch (e: Exception) {
                // IOException, IllegalStateException, SecurityException: a file that does not play.
                val why = e.javaClass.simpleName
                onView { fail(why) }
            }
        }

        /** Runs [block] on the main thread while [view] still plays this player. */
        private fun onView(block: LoopingVideoView.() -> Unit) {
            if (released) return
            main.post { if (!released && view.player === this) view.block() }
        }
    }

    companion object {
        private const val TAG = "stickpile.motion"

        /** Videos playing at once in the process: each holds a decoder, of which a phone has few. */
        const val MAX_PLAYING = 6

        private val slots = Slots<LoopingVideoView>(MAX_PLAYING)

        private val main = Handler(Looper.getMainLooper())

        /** The players' thread: made, started and released there, in order. */
        private val worker: Handler by lazy {
            Handler(HandlerThread("stickpile-video").apply { start() }.looper)
        }
    }
}
