package io.github.dolfost.stickpile.provider

import io.github.dolfost.stickpile.provider.ProviderAccess.Decision
import org.junit.Assert.assertEquals
import org.junit.Test

class ProviderAccessTest {
    private val own = 10_100
    private val keyboard = 10_200
    private val chat = 10_300
    private val shared = 10_400
    private val nobody = 10_500

    private val packages = mapOf(
        own to arrayOf("io.github.dolfost.stickpile"),
        keyboard to arrayOf("juloo.keyboard2"),
        chat to arrayOf("org.telegram.messenger"),
        shared to arrayOf("com.example.one", "com.example.two"),
    )

    private fun decide(uid: Int, approved: Set<String>, callingPackage: String? = null) =
        ProviderAccess.decide(uid, own, callingPackage, { packages[it] }, { it in approved })

    @Test
    fun theAppItselfIsAlwaysAllowed() {
        assertEquals(Decision.Allow, decide(own, emptySet()))
        assertEquals(Decision.Allow, decide(own, emptySet(), "io.github.dolfost.stickpile"))
    }

    @Test
    fun anApprovedKeyboardIsAllowed() {
        assertEquals(Decision.Allow, decide(keyboard, setOf("juloo.keyboard2")))
        assertEquals(Decision.Allow, decide(keyboard, setOf("juloo.keyboard2"), "juloo.keyboard2"))
    }

    @Test
    fun oneApprovedPackageOfASharedUidIsEnough() {
        assertEquals(Decision.Allow, decide(shared, setOf("com.example.two")))
    }

    @Test
    fun anyoneElseIsRefusedAndNoted() {
        assertEquals(Decision.Deny(listOf("org.telegram.messenger")), decide(chat, setOf("juloo.keyboard2")))
        assertEquals(Decision.Deny(listOf("juloo.keyboard2")), decide(keyboard, emptySet()))
        assertEquals(
            Decision.Deny(listOf("com.example.one", "com.example.two")),
            decide(shared, setOf("org.telegram.messenger")),
        )
        // The calling package comes first and is not repeated.
        assertEquals(
            Decision.Deny(listOf("com.example.two", "com.example.one")),
            decide(shared, emptySet(), "com.example.two"),
        )
    }

    @Test
    fun aUidWithoutPackagesIsRefusedWithNothingToNote() {
        assertEquals(Decision.Deny(emptyList()), decide(nobody, setOf("juloo.keyboard2")))
    }

    @Test
    fun theCheckedCallingPackageCountsWhenThePackageListIsHidden() {
        assertEquals(Decision.Allow, decide(nobody, setOf("juloo.keyboard2"), "juloo.keyboard2"))
        assertEquals(Decision.Deny(listOf("juloo.keyboard2")), decide(nobody, emptySet(), "juloo.keyboard2"))
    }

    @Test
    fun approvingAnotherAppsPackageDoesNotHelp() {
        // The chat app's uid does not become a keyboard's because the keyboard is approved.
        assertEquals(Decision.Deny(listOf("org.telegram.messenger")), decide(chat, setOf("juloo.keyboard2")))
    }
}
