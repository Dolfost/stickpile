package io.github.dolfost.stickpile.offline

import android.content.Context
import android.content.SharedPreferences
import androidx.core.content.edit
import io.github.dolfost.stickpile.rust.OfflineOptions
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * What the app keeps of keeping stickers offline (docs/offline.md): the
 * options the library gets ([OfflineOptions]; `OfflineOptions()` is the
 * core's defaults) and whether the downloads are paused. What is kept (a
 * sticker, a pack, the library) is the library's, in its cache.
 */
data class OfflineChoices(
    val options: OfflineOptions = OfflineOptions(),
    /** The downloads wait until resumed, also after a restart. */
    val paused: Boolean = false,
)

/** Where [OfflineChoices] are kept; [OfflineSettings] on the phone, a fake in tests. */
interface OfflineStore {
    val state: StateFlow<OfflineChoices>
    val current: OfflineChoices get() = state.value

    fun setOptions(options: OfflineOptions)

    fun setPaused(paused: Boolean)
}

/** [OfflineChoices] in memory only (tests, and screens made without the app's). */
class MemoryOfflineStore(initial: OfflineChoices = OfflineChoices()) : OfflineStore {
    private val mutable = MutableStateFlow(initial)
    override val state: StateFlow<OfflineChoices> = mutable.asStateFlow()

    override fun setOptions(options: OfflineOptions) {
        mutable.value = current.copy(options = options)
    }

    override fun setPaused(paused: Boolean) {
        mutable.value = current.copy(paused = paused)
    }
}

/**
 * [OfflineChoices] as preference values and back (pure). Each option is a
 * key of its own; a key missing or of the wrong type is the default, so an
 * option added later starts as the core says.
 */
object OfflinePrefs {
    const val STICKER_RENDITIONS = "sticker_renditions"
    const val STILLS = "stills"
    const val ANIMATIONS = "animations"
    const val VIDEOS = "videos"
    const val MAX_FILE_MB = "max_file_mb"
    const val MAX_MB = "max_mb"
    const val AUTO_THUMBNAILS = "auto_thumbnails"
    const val AUTO_USED = "auto_used"
    const val AUTO_USED_COUNT = "auto_used_count"
    const val AUTO_LIKED = "auto_liked"
    const val AUTO_OWN_PACKS = "auto_own_packs"
    const val MOBILE_DATA = "mobile_data"
    const val PAUSED = "paused"

    /** The most `auto_used_count` (the copy history keeps 2000). */
    const val USED_MAX = 2000

    /** The values to store for [choices]. */
    fun write(choices: OfflineChoices): Map<String, Any> {
        val o = choices.options
        return mapOf(
            STICKER_RENDITIONS to o.stickerRenditions,
            STILLS to o.stills,
            ANIMATIONS to o.animations,
            VIDEOS to o.videos,
            MAX_FILE_MB to o.maxFileMb.toLong(),
            MAX_MB to o.maxMb.toLong(),
            AUTO_THUMBNAILS to o.autoThumbnails,
            AUTO_USED to o.autoUsed,
            AUTO_USED_COUNT to o.autoUsedCount.toInt(),
            AUTO_LIKED to o.autoLiked,
            AUTO_OWN_PACKS to o.autoOwnPacks,
            MOBILE_DATA to o.mobileData,
            PAUSED to choices.paused,
        )
    }

    /** The choices stored as [values] (`SharedPreferences.getAll`). */
    fun read(values: Map<String, *>): OfflineChoices {
        val d = OfflineOptions()
        fun bool(key: String, default: Boolean) = values[key] as? Boolean ?: default
        fun mb(key: String, default: ULong) = (values[key] as? Long)?.takeIf { it >= 0 }?.toULong() ?: default
        val used = (values[AUTO_USED_COUNT] as? Int)?.coerceIn(1, USED_MAX)?.toUInt() ?: d.autoUsedCount
        return OfflineChoices(
            options = OfflineOptions(
                stickerRenditions = bool(STICKER_RENDITIONS, d.stickerRenditions),
                stills = bool(STILLS, d.stills),
                animations = bool(ANIMATIONS, d.animations),
                videos = bool(VIDEOS, d.videos),
                maxFileMb = mb(MAX_FILE_MB, d.maxFileMb),
                maxMb = mb(MAX_MB, d.maxMb),
                autoThumbnails = bool(AUTO_THUMBNAILS, d.autoThumbnails),
                autoUsed = bool(AUTO_USED, d.autoUsed),
                autoUsedCount = used,
                autoLiked = bool(AUTO_LIKED, d.autoLiked),
                autoOwnPacks = bool(AUTO_OWN_PACKS, d.autoOwnPacks),
                mobileData = bool(MOBILE_DATA, d.mobileData),
            ),
            paused = bool(PAUSED, false),
        )
    }
}

/**
 * Keeps [OfflineChoices] in the app's private preferences, next to the
 * display settings ([io.github.dolfost.stickpile.display.DisplaySettings]).
 * One per process ([io.github.dolfost.stickpile.StickpileApp.offline]).
 * The caller hands the options to the library and the job
 * ([OfflineScheduler]) when they change.
 */
class OfflineSettings(context: Context) : OfflineStore {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private val mutable = MutableStateFlow(OfflinePrefs.read(prefs.all))

    override val state: StateFlow<OfflineChoices> = mutable.asStateFlow()

    // Kept in a field: the preferences hold their listeners weakly.
    private val listener = SharedPreferences.OnSharedPreferenceChangeListener { p, _ ->
        mutable.value = OfflinePrefs.read(p.all)
    }

    init {
        prefs.registerOnSharedPreferenceChangeListener(listener)
    }

    override fun setOptions(options: OfflineOptions) = save(current.copy(options = options))

    override fun setPaused(paused: Boolean) = save(current.copy(paused = paused))

    private fun save(choices: OfflineChoices) {
        mutable.value = choices
        prefs.edit {
            for ((key, value) in OfflinePrefs.write(choices)) {
                when (value) {
                    is Boolean -> putBoolean(key, value)
                    is Long -> putLong(key, value)
                    is Int -> putInt(key, value)
                }
            }
        }
    }

    private companion object {
        const val PREFS = "offline"
    }
}
