package io.github.dolfost.stickpile

import android.app.Activity
import android.app.Application
import android.app.Service
import android.content.BroadcastReceiver
import android.content.ContentProvider
import io.github.dolfost.stickpile.provider.StickerProvider
import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element

/**
 * The promises the manifest makes (docs/android.md): which permissions the
 * app asks for, that its data is never backed up or transferred, how the
 * provider and the input method are exposed, and that every component it
 * names exists. Reads the source files (the test runs in android/app).
 */
class ManifestTest {
    private val androidNs = "http://schemas.android.com/apk/res/android"
    private val main = File("src/main")
    private val manifest = parse(File(main, "AndroidManifest.xml"))
    private val application = manifest.children("application").single()

    @Test
    fun asksForTheInternetAndTheCameraOnly() {
        val permissions = manifest.children("uses-permission").map { it.android("name") }
        assertEquals(listOf("android.permission.INTERNET", "android.permission.CAMERA"), permissions)
        val camera = manifest.children("uses-feature").single { it.android("name") == "android.hardware.camera" }
        assertEquals("false", camera.android("required"))
    }

    @Test
    fun nothingIsBackedUpOrTransferred() {
        assertEquals("false", application.android("allowBackup"))
        assertEquals("@xml/data_extraction_rules", application.android("dataExtractionRules"))
        assertEquals("@xml/backup_rules", application.android("fullBackupContent"))

        val rules = parse(File(main, "res/xml/data_extraction_rules.xml"))
        val sections = rules.children()
        assertEquals(listOf("cloud-backup", "device-transfer"), sections.map { it.tagName })
        sections.forEach(::assertExcludesEverything)
        assertExcludesEverything(parse(File(main, "res/xml/backup_rules.xml")))
    }

    @Test
    fun theProviderIsExportedUnderTheContractsAuthorityWithoutGrants() {
        val provider = component("provider", ".provider.StickerProvider")
        assertEquals("io.github.dolfost.stickpile.stickers", StickerProvider.AUTHORITY)
        assertEquals(StickerProvider.AUTHORITY, provider.android("authorities"))
        assertEquals("true", provider.android("exported"))
        assertEquals("false", provider.android("grantUriPermissions"))
        // Access control is in code, never a manifest permission.
        listOf("permission", "readPermission", "writePermission").forEach {
            assertFalse("provider has android:$it", provider.hasAttributeNS(androidNs, it))
        }
        assertTrue(provider.children("grant-uri-permission").isEmpty())
        assertTrue(provider.children("path-permission").isEmpty())
    }

    @Test
    fun onlyTheSystemCanBindTheInputMethod() {
        val ime = component("service", ".ime.PickerInputMethodService")
        assertEquals("android.permission.BIND_INPUT_METHOD", ime.android("permission"))
        assertEquals("@string/ime_name", ime.android("label"))
        val actions = ime.children("intent-filter").flatMap { it.children("action") }.map { it.android("name") }
        assertEquals(listOf("android.view.InputMethod"), actions)
        val meta = ime.children("meta-data").single()
        assertEquals("android.view.im", meta.android("name"))
        assertEquals("@xml/method", meta.android("resource"))

        val method = parse(File(main, "res/xml/method.xml"))
        assertEquals("io.github.dolfost.stickpile.ui.SettingsActivity", method.android("settingsActivity"))
        assertEquals("true", method.android("supportsSwitchingToNextInputMethod"))
        assertEquals("@string/ime_subtype_label", method.children("subtype").single().android("label"))
    }

    @Test
    fun theShareTargetTakesImagesAndVideos() {
        val share = component("activity", ".share.ShareActivity")
        val filters = share.children("intent-filter").associate { filter ->
            filter.children("action").single().android("name") to
                filter.children("data").map { it.android("mimeType") }
        }
        val media = listOf("image/*", "video/*")
        assertEquals(
            mapOf("android.intent.action.SEND" to media, "android.intent.action.SEND_MULTIPLE" to media),
            filters,
        )
    }

    @Test
    fun everyComponentHasItsClass() {
        val types: Map<String, Class<*>> = mapOf(
            "activity" to Activity::class.java,
            "service" to Service::class.java,
            "provider" to ContentProvider::class.java,
            "receiver" to BroadcastReceiver::class.java,
        )
        val named: List<Pair<String, Class<*>>> =
            listOf(application.android("name") to Application::class.java) +
                types.flatMap { (tag, type) -> application.children(tag).map { it.android("name") to type } }
        assertEquals(6, named.size)
        named.forEach { (name, type) ->
            val cls = Class.forName(qualified(name), false, javaClass.classLoader)
            assertTrue("$name is not a ${type.simpleName}", type.isAssignableFrom(cls))
        }
    }

    private fun assertExcludesEverything(section: Element) {
        val domains = listOf(
            "root", "file", "database", "sharedpref", "external",
            "device_root", "device_file", "device_database", "device_sharedpref",
        )
        assertTrue("${section.tagName} includes something", section.children("include").isEmpty())
        val excluded = section.children("exclude").filter { it.getAttribute("path") == "." }
        assertEquals(section.tagName, domains.toSet(), excluded.map { it.getAttribute("domain") }.toSet())
    }

    private fun component(tag: String, name: String): Element =
        application.children(tag).single { it.android("name") == name }

    private fun qualified(name: String) = if (name.startsWith(".")) "io.github.dolfost.stickpile$name" else name

    private fun Element.android(name: String): String {
        assertTrue("<$tagName> has no android:$name", hasAttributeNS(androidNs, name))
        return getAttributeNS(androidNs, name)
    }

    private fun Element.children(tag: String? = null): List<Element> {
        val nodes = childNodes
        return (0 until nodes.length).map { nodes.item(it) }.filterIsInstance<Element>()
            .filter { tag == null || it.tagName == tag }
    }

    private fun parse(file: File): Element {
        val factory = DocumentBuilderFactory.newInstance().apply { isNamespaceAware = true }
        return factory.newDocumentBuilder().parse(file).documentElement
    }
}
