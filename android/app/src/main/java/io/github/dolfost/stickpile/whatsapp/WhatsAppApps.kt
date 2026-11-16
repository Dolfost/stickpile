package io.github.dolfost.stickpile.whatsapp

import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.content.pm.ProviderInfo
import android.net.Uri
import android.os.Build

/** Which WhatsApp apps are installed and whether a part is added to them. An interface so that tests need no Android. */
fun interface WhatsAppApps {
    /** For the part [identifier]. Asks the package manager and WhatsApp's providers: call it off the main thread. */
    fun presence(identifier: String): WhatsAppPresence
}

/**
 * [WhatsAppApps] on the phone, as the sample's `WhitelistCheck` asks: an
 * app counts when it is installed and enabled, and a part is added to it
 * when its `sticker_whitelist_check` provider answers 1 for this app's
 * authority and the part's identifier. An app without that provider (too
 * old) or that fails to answer has not added it. The manifest's
 * `<queries>` makes both apps visible to the package manager.
 */
class AndroidWhatsAppApps(private val context: Context) : WhatsAppApps {
    override fun presence(identifier: String): WhatsAppPresence {
        val consumer = installed(WhatsApp.CONSUMER)
        val business = installed(WhatsApp.BUSINESS)
        return WhatsAppPresence(
            consumerInstalled = consumer,
            businessInstalled = business,
            inConsumer = consumer && added(WhatsApp.CONSUMER, identifier),
            inBusiness = business && added(WhatsApp.BUSINESS, identifier),
        )
    }

    private fun installed(packageName: String): Boolean = try {
        appInfo(packageName).enabled
    } catch (_: PackageManager.NameNotFoundException) {
        false
    }

    private fun added(packageName: String, identifier: String): Boolean {
        val authority = packageName + WhatsApp.WHITELIST_PROVIDER
        if (provider(authority) == null) return false
        val uri = Uri.Builder()
            .scheme(ContentResolver.SCHEME_CONTENT)
            .authority(authority)
            .appendPath(WhatsApp.WHITELIST_PATH)
            .appendQueryParameter(WhatsApp.WHITELIST_AUTHORITY_PARAM, WhatsAppRoutes.AUTHORITY)
            .appendQueryParameter(WhatsApp.WHITELIST_IDENTIFIER_PARAM, identifier)
            .build()
        return try {
            context.contentResolver.query(uri, null, null, null, null)?.use { cursor ->
                cursor.moveToFirst() && cursor.getInt(cursor.getColumnIndexOrThrow(WhatsApp.WHITELIST_RESULT)) == 1
            } ?: false
        } catch (_: Exception) {
            false
        }
    }

    private fun appInfo(packageName: String): ApplicationInfo {
        val pm = context.packageManager
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.getApplicationInfo(packageName, PackageManager.ApplicationInfoFlags.of(0))
        } else {
            @Suppress("DEPRECATION")
            pm.getApplicationInfo(packageName, 0)
        }
    }

    private fun provider(authority: String): ProviderInfo? {
        val pm = context.packageManager
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.resolveContentProvider(authority, PackageManager.ComponentInfoFlags.of(0))
        } else {
            @Suppress("DEPRECATION")
            pm.resolveContentProvider(authority, 0)
        }
    }
}

/**
 * The intent of [request]: the action and the three extras, aimed at its
 * app, or a chooser titled [chooserTitle] when either may take it (the
 * sample's `launchIntentToAddPackToChooser`, so that both apps are
 * offered even when one is the default).
 */
fun AddRequest.intent(chooserTitle: String): Intent {
    val add = Intent(action)
    extras.forEach { (key, value) -> add.putExtra(key, value) }
    val target = packageName ?: return Intent.createChooser(add, chooserTitle)
    return add.setPackage(target)
}
