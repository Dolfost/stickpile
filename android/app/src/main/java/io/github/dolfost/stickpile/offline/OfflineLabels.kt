package io.github.dolfost.stickpile.offline

import androidx.annotation.StringRes
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.KeepReason
import io.github.dolfost.stickpile.rust.OfflineEstimate
import io.github.dolfost.stickpile.rust.OfflineScope

/**
 * A question before keeping a pack or the library: [estimate] is what it
 * takes (null while it is counted); [name] the pack's (null: the library).
 */
data class KeepAsk(val scope: OfflineScope, val name: String? = null, val estimate: OfflineEstimate? = null)

/** What a sticker's or a pack's menu offers. */
enum class KeepAction {
    /** Keep it offline. */
    KEEP,

    /** Stop keeping it offline (it is kept for itself). */
    RELEASE,
}

/**
 * The words of keeping offline in the menus of the app and the keyboard
 * (pure): which action a menu offers, its label, and why a sticker is
 * kept.
 */
object OfflineLabels {
    /**
     * The action of a sticker kept for [reasons]: released only when it is
     * kept by itself; kept with its pack or a rule, keeping it by itself is
     * still offered (it then stays when the pack goes).
     */
    fun stickerAction(reasons: List<KeepReason>): KeepAction =
        if (KeepReason.STICKER in reasons) KeepAction.RELEASE else KeepAction.KEEP

    /** The action of a pack, or the library, that is [kept] or not. */
    fun scopeAction(kept: Boolean): KeepAction = if (kept) KeepAction.RELEASE else KeepAction.KEEP

    /** Whether [scope] is among [keeps]. */
    fun kept(scope: OfflineScope, keeps: List<OfflineScope>): Boolean = scope in keeps

    @StringRes
    fun label(action: KeepAction): Int = when (action) {
        KeepAction.KEEP -> R.string.offline_keep
        KeepAction.RELEASE -> R.string.offline_release
    }

    @StringRes
    fun reason(reason: KeepReason): Int = when (reason) {
        KeepReason.STICKER -> R.string.offline_reason_sticker
        KeepReason.PACK -> R.string.offline_reason_pack
        KeepReason.LIBRARY -> R.string.offline_reason_library
        KeepReason.USED -> R.string.offline_reason_used
        KeepReason.LIKED -> R.string.offline_reason_liked
        KeepReason.OWN_PACK -> R.string.offline_reason_own_pack
    }

    /** The reasons' words in their order, for "Available offline: ..."; empty when not kept. */
    fun reasons(reasons: List<KeepReason>): List<Int> = reasons.distinct().map(::reason)
}

/**
 * The steps of the settings' sliders, in MiB (0: no limit, at the end of
 * the budget's and the start of the largest file's), and the counts of
 * stickers sent lately (pure). A value set elsewhere shows at its nearest
 * step.
 */
object OfflineSizes {
    val MAX_FILE_MB: List<ULong> = listOf(0uL, 1uL, 2uL, 5uL, 10uL, 20uL, 50uL, 100uL)
    val BUDGET_MB: List<ULong> = listOf(256uL, 512uL, 1024uL, 2048uL, 5120uL, 10240uL, 20480uL, 0uL)
    val USED_COUNT: List<UInt> = listOf(25u, 50u, 100u, 200u, 500u, 1000u, 2000u)

    /** The step of [steps] showing [value]: itself, else the nearest (no limit counts as the most). */
    fun index(steps: List<ULong>, value: ULong): Int {
        val exact = steps.indexOf(value)
        if (exact >= 0) return exact
        fun weight(v: ULong) = if (v == 0uL) ULong.MAX_VALUE else v
        return steps.indices.minBy { i ->
            val a = weight(steps[i])
            val b = weight(value)
            if (a > b) a - b else b - a
        }
    }

    fun usedIndex(value: UInt): Int = index(USED_COUNT.map { it.toULong() }, value.toULong())
}
