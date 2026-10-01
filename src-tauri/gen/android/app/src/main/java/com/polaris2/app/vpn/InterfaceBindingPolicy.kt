package com.polaris2.app.vpn

import java.io.Closeable
import java.io.IOException

internal data class InterfaceBindingCandidate<N>(
    val network: N,
    val interfaceName: String,
    val physical: Boolean,
)

/** Restricted IMS/MMS Networks are visible to ConnectivityManager but unavailable to ordinary apps.
 * INTERNET is deliberately not required: an unrestricted local LAN is still a valid explicit exit.
 */
internal fun isBindablePhysicalNetwork(notVpn: Boolean, vpnTransport: Boolean, notRestricted: Boolean): Boolean =
    notVpn && !vpnTransport && notRestricted

internal class InterfaceBindingFailure(val code: String) : IOException("android: $code")

internal fun <N> requireInterfaceNetwork(
    name: String,
    candidates: List<InterfaceBindingCandidate<N>>,
): N {
    if (name.isBlank() || name != name.trim()) throw InterfaceBindingFailure("BIND_INTERFACE_INVALID")
    val matches = candidates.filter { it.physical && it.interfaceName == name }
    if (matches.isEmpty()) throw InterfaceBindingFailure("BIND_INTERFACE_UNAVAILABLE")
    if (matches.size != 1) throw InterfaceBindingFailure("BIND_INTERFACE_AMBIGUOUS")
    return matches.single().network
}

/** The caller owns fd. Only its borrowed duplicate is closed, including on every failed stage. */
internal fun <N, F : Closeable> bindInterfaceSocket(
    fd: Int,
    name: String,
    candidates: () -> List<InterfaceBindingCandidate<N>>,
    borrow: (Int) -> F,
    protect: (Int) -> Unit,
    bind: (N, F) -> Unit,
) {
    if (fd < 0) throw InterfaceBindingFailure("BIND_INTERFACE_BAD_FD")
    val selected = requireInterfaceNetwork(name, candidates())
    borrow(fd).use { duplicate ->
        if (requireInterfaceNetwork(name, candidates()) != selected) {
            throw InterfaceBindingFailure("BIND_INTERFACE_NETWORK_CHANGED")
        }
        protect(fd)
        bind(selected, duplicate)
    }
}
