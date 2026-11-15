package io.github.dolfost.stickpile.provider

/**
 * Who may use [StickerProvider] (docs/android.md, "Provider contract",
 * "Access control"), as a pure decision: the app's own uid always; any
 * other uid only when one of its packages is approved in
 * [KeyboardAllowlist]. `/prepared/` files are not decided here: their token
 * is the capability.
 */
object ProviderAccess {
    sealed interface Decision {
        data object Allow : Decision

        /** Refuse, and note these packages (the caller's) as pending. */
        data class Deny(val packages: List<String>) : Decision
    }

    /**
     * @param callingUid `Binder.getCallingUid()`.
     * @param ownUid `Process.myUid()`.
     * @param callingPackage `ContentProvider.getCallingPackage()`, which the
     *   system has checked to belong to [callingUid]; null where there is
     *   none (`getType`).
     * @param packagesOf `PackageManager.getPackagesForUid`; null when the uid has none.
     * @param isApproved whether the user approved a package (and it is still the app approved).
     */
    fun decide(
        callingUid: Int,
        ownUid: Int,
        callingPackage: String?,
        packagesOf: (Int) -> Array<String>?,
        isApproved: (String) -> Boolean,
    ): Decision {
        if (callingUid == ownUid) return Decision.Allow
        val packages = (listOfNotNull(callingPackage) + packagesOf(callingUid).orEmpty()).distinct()
        return if (packages.any(isApproved)) Decision.Allow else Decision.Deny(packages)
    }
}
