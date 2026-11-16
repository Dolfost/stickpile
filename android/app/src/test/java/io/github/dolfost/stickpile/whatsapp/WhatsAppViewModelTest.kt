package io.github.dolfost.stickpile.whatsapp

import android.app.Activity
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.WhatsappPacks
import io.github.dolfost.stickpile.rust.WhatsappProgress
import io.github.dolfost.stickpile.rust.WhatsappStep
import io.github.dolfost.stickpile.ui.FakeLibrary
import io.github.dolfost.stickpile.ui.MainDispatcherRule
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.pack
import io.github.dolfost.stickpile.ui.unreachable
import io.github.dolfost.stickpile.ui.whatsappPart
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** "Add to WhatsApp": the sheet's states, the dialogs it opens and what they answer. */
@OptIn(ExperimentalCoroutinesApi::class)
class WhatsAppViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val library = FakeLibrary()
    private val cats = pack("p1")

    /** What the phone has: WhatsApp alone by default, with no pack added. */
    private var presence = WhatsAppPresence(consumerInstalled = true, businessInstalled = false)
    private val asked = mutableListOf<String>()
    private val traced = mutableListOf<String>()
    private val vm by lazy {
        WhatsAppViewModel(
            library,
            apps = { identifier ->
                asked += identifier
                presence
            },
            io = main.dispatcher,
            log = { _, _ -> },
            trace = { traced += it },
        )
    }

    private fun packs(vararg parts: Int, offline: Boolean = false, skipped: Int = 0) = WhatsappPacks(
        parts = parts.map { whatsappPart("p1", it) },
        skipped = skipped.toUInt(),
        offline = offline,
    )

    private suspend fun TestScope.nextLaunch(): AddRequest =
        (vm.eventFlow.first() as WhatsAppEvent.Launch).request

    private fun ready() = vm.state as WhatsAppState.Ready

    @Test
    fun theProgressShowsWhileThePackIsMadeReady() = runTest(main.dispatcher) {
        val answer = CompletableDeferred<WhatsappPacks>()
        library.whatsappPacks = { answer.await() }
        var progress: WhatsappProgress? = null
        library.whatsappProgress = { progress }

        vm.open(cats)
        runCurrent()
        assertEquals(WhatsAppState.Working(cats, WhatsappStep.BUILDING), vm.state)
        assertEquals(listOf("whatsappPacks p1"), library.calls)

        progress = WhatsappProgress(WhatsappStep.DOWNLOADING, 2u, 3u)
        advanceTimeBy(WhatsAppViewModel.PROGRESS_EVERY_MS + 1)
        assertEquals(WhatsAppState.Working(cats, WhatsappStep.DOWNLOADING, 2, 3), vm.state)

        answer.complete(packs(1, 2, 3))
        runCurrent()
        val ready = ready()
        assertEquals(listOf(1, 2, 3), ready.parts.map { it.part.number.toInt() })
        assertTrue(ready.parts.all { it.status == PartStatus.NOT_ADDED })
        assertEquals(listOf("p1-1", "p1-2", "p1-3"), asked)
        // Several parts: none is added before the user taps its button.
        advanceTimeBy(10 * WhatsAppViewModel.PROGRESS_EVERY_MS)
        assertTrue(traced.isEmpty())
    }

    @Test
    fun aPackOfOnePartOpensWhatsAppsDialogAtOnce() = runTest(main.dispatcher) {
        library.whatsappPacks = { packs(1) }
        vm.open(cats)
        val request = nextLaunch()
        assertEquals(AddRequest("p1-1", "Pack p1", packageName = "com.whatsapp"), request)
        assertEquals(
            mapOf(
                "sticker_pack_id" to "p1-1",
                "sticker_pack_authority" to "io.github.dolfost.stickpile.whatsapp",
                "sticker_pack_name" to "Pack p1",
            ),
            request.extras,
        )
        assertEquals(PartStatus.ADDING, ready().parts.single().status)
        assertTrue(traced.single().startsWith("asking WhatsApp: com.whatsapp.intent.action.ENABLE_STICKER_PACK"))

        vm.added(Activity.RESULT_OK, null)
        assertEquals(PartStatus.ADDED, ready().parts.single().status)
    }

    @Test
    fun eachPartIsAddedOnItsOwnAndOneAtATime() = runTest(main.dispatcher) {
        library.whatsappPacks = { packs(1, 2) }
        presence = WhatsAppPresence(consumerInstalled = true, businessInstalled = true, inConsumer = true)
        vm.open(cats)
        advanceUntilIdle()

        vm.add("p1-2")
        vm.add("p1-1") // while the first dialog is open: nothing
        val request = nextLaunch()
        assertEquals(AddRequest("p1-2", "Pack p1", packageName = "com.whatsapp.w4b"), request)
        assertEquals(listOf(PartStatus.NOT_ADDED, PartStatus.ADDING), ready().parts.map { it.status })

        // Closed without adding.
        vm.added(Activity.RESULT_CANCELED, null)
        assertEquals(listOf(PartStatus.NOT_ADDED, PartStatus.CANCELLED), ready().parts.map { it.status })
        // Refused by WhatsApp's checks: the reason goes to the log.
        vm.add("p1-1")
        nextLaunch()
        vm.added(Activity.RESULT_CANCELED, "sticker pack sticker count should be between 3 to 30")
        assertEquals(listOf(PartStatus.REFUSED, PartStatus.CANCELLED), ready().parts.map { it.status })
        assertTrue(traced.last(), traced.last().contains("refused p1-1"))
    }

    @Test
    fun bothAppsWithoutThePartGetAChooser() = runTest(main.dispatcher) {
        library.whatsappPacks = { packs(1) }
        presence = WhatsAppPresence(consumerInstalled = true, businessInstalled = true)
        vm.open(cats)
        assertEquals(AddRequest("p1-1", "Pack p1", packageName = null), nextLaunch())
    }

    @Test
    fun partsWhatsAppHasAlreadyAreMarked() = runTest(main.dispatcher) {
        library.whatsappPacks = { packs(1) }
        presence = WhatsAppPresence(consumerInstalled = true, businessInstalled = false, inConsumer = true)
        vm.open(cats)
        advanceUntilIdle()
        assertEquals(PartStatus.ALREADY_ADDED, ready().parts.single().status)
        assertTrue("no dialog", traced.isEmpty())
    }

    @Test
    fun withoutWhatsAppTheFilesAreMadeAndTheSheetSaysSo() = runTest(main.dispatcher) {
        library.whatsappPacks = { packs(1, offline = true, skipped = 2) }
        presence = WhatsAppPresence(consumerInstalled = false, businessInstalled = false)
        vm.open(cats)
        advanceUntilIdle()
        val ready = ready()
        assertTrue(ready.offline)
        assertEquals(2, ready.skipped)
        assertEquals(Message(R.string.whatsapp_not_installed), ready.notice)
        assertEquals(PartStatus.NOT_ADDED, ready.parts.single().status)
        // The intent it would have sent is in the log.
        assertTrue(traced.single(), traced.single().contains("sticker_pack_id=p1-1"))

        // WhatsApp went between the check and the dialog.
        presence = WhatsAppPresence(consumerInstalled = true, businessInstalled = false)
        vm.add("p1-1")
        nextLaunch()
        vm.notStarted()
        assertEquals(PartStatus.NOT_ADDED, ready().parts.single().status)
        assertEquals(Message(R.string.whatsapp_not_installed), ready().notice)
    }

    @Test
    fun failuresHaveTheirOwnTexts() = runTest(main.dispatcher) {
        val cases = listOf(
            LibraryException.NotFound("gone") to R.string.whatsapp_error_pack_gone,
            LibraryException.Server(404u, "old") to R.string.whatsapp_error_server_too_old,
            LibraryException.InvalidInput("pack", "nothing to export") to R.string.whatsapp_error_too_small,
            unreachable() to R.string.whatsapp_error_offline,
            LibraryException.Other("broken") to R.string.whatsapp_error_failed,
            LibraryException.Server(500u, "d") to R.string.app_error_server,
            LibraryException.NotSignedIn("d") to R.string.app_error_not_signed_in,
        )
        for ((e, text) in cases) {
            library.whatsappPacks = { throw e }
            vm.open(cats)
            advanceUntilIdle()
            val failed = vm.state as WhatsAppState.Failed
            assertEquals(e.toString(), text, failed.message.id)
        }
        // Again: a retry asks the library again.
        library.whatsappPacks = { packs(1, 2) }
        library.calls.clear()
        vm.retry()
        advanceUntilIdle()
        assertEquals(listOf("whatsappPacks p1"), library.calls)
        assertEquals(2, ready().parts.size)
    }

    @Test
    fun closingStopsWaiting() = runTest(main.dispatcher) {
        val answer = CompletableDeferred<WhatsappPacks>()
        var cancelled = false
        library.whatsappPacks = {
            try {
                answer.await()
            } catch (e: kotlinx.coroutines.CancellationException) {
                cancelled = true
                throw e
            }
        }
        vm.open(cats)
        runCurrent()
        vm.close()
        runCurrent()
        assertEquals(WhatsAppState.Closed, vm.state)
        assertTrue(cancelled)
    }
}
