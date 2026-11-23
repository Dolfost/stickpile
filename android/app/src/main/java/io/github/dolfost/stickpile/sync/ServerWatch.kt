package io.github.dolfost.stickpile.sync

import io.github.dolfost.stickpile.rust.ServerReach

/**
 * Follows what the library knows of the server (docs/offline.md,
 * "Reachability and results without waiting") for a screen that shows
 * results: the app's library and the keyboard. Pure: the library's calls
 * come in as functions.
 *
 * [now] is the value when the screen starts following; [change] is the
 * library's long poll (`serverReachChange`), which answers once the value
 * differs from the one it is given. [back] runs each time the server
 * answers again after it was found away (`Unreachable` seen since the last
 * `Reachable`, an `Unknown` between them or not): the screen runs what it
 * shows again in place, which drops its offline notice, and a sync starts.
 * It also runs when the server answers while [stale] says that what is
 * shown came from the offline cache, at the start too: a list that was
 * read while the server was away is read again once it is back, however
 * the watch learnt it. Runs until cancelled; a failure of [change] ends it
 * (thrown).
 */
object ServerWatch {
    suspend fun follow(
        now: ServerReach,
        change: suspend (ServerReach) -> ServerReach,
        stale: () -> Boolean,
        back: suspend () -> Unit,
    ) {
        var seen = now
        var away = now is ServerReach.Unreachable
        if (now == ServerReach.Reachable && stale()) back()
        while (true) {
            seen = change(seen)
            when (seen) {
                is ServerReach.Unreachable -> away = true
                ServerReach.Reachable -> if (away || stale()) {
                    away = false
                    back()
                }
                ServerReach.Unknown -> Unit
            }
        }
    }
}
