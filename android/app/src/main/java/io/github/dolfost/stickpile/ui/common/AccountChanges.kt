package io.github.dolfost.stickpile.ui.common

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/**
 * Counts the account changes made away from the launcher (a sign-out in
 * the settings, a change of server during onboarding), for this process.
 * The launcher watches [version] while it is resumed and asks again who
 * is signed in when it moved, so that a sign-out that ended after its
 * screen went still shows onboarding.
 */
object AccountChanges {
    private val counter = MutableStateFlow(0)

    val version: StateFlow<Int> = counter.asStateFlow()

    /** The account changed (signed out, or another server). */
    fun changed() {
        counter.update { it + 1 }
    }
}
