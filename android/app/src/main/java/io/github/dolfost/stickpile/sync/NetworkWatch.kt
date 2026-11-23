package io.github.dolfost.stickpile.sync

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.util.Log

/**
 * Asks for a check of the server ([check]: `StickpileLibrary.checkServer`)
 * whenever a network appears: the default network changes (Wi-Fi, mobile
 * data, a VPN such as Tailscale coming up), and once the platform has
 * validated it. A server found away is then found again as soon as the
 * phone can reach it, without waiting for the background check's next
 * turn (docs/offline.md). One callback for the process, which hosts the app
 * and the keyboard, registered when the library opens; it reads only that
 * a network is there (`ACCESS_NETWORK_STATE`, which WorkManager already puts
 * in the merged manifest). [check] runs on a thread of the platform's: it
 * must return at once.
 */
object NetworkWatch {
    private const val TAG = "stickpile"

    fun start(context: Context, check: () -> Unit) {
        val connectivity = context.getSystemService(ConnectivityManager::class.java) ?: return
        try {
            connectivity.registerDefaultNetworkCallback(Callback(check))
        } catch (e: RuntimeException) {
            // A platform that refuses it (SecurityException, too many
            // callbacks): the background checks still find the server.
            Log.w(TAG, "no network callback: ${e.javaClass.simpleName}")
        }
    }

    private class Callback(private val check: () -> Unit) : ConnectivityManager.NetworkCallback() {
        /** The network last seen validated: each one is checked for once more when it is. */
        @Volatile
        private var validated: Network? = null

        override fun onAvailable(network: Network) = check()

        override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
            if (capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) && validated != network) {
                validated = network
                check()
            }
        }
    }
}
