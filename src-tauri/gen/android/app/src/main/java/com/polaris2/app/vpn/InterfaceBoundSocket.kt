package com.polaris2.app.vpn

import android.net.Network
import android.net.NetworkCapabilities
import android.os.ParcelFileDescriptor
import java.io.FileDescriptor

/** Explicit socket binding, separate from the default-interface monitor and process routing. */
internal object InterfaceBoundSocket {
    fun candidates(): List<InterfaceBindingCandidate<Network>> {
        val connectivity = PolarisApplication.connectivity
        @Suppress("DEPRECATION")
        return connectivity.allNetworks.mapNotNull { network ->
            val capabilities = connectivity.getNetworkCapabilities(network) ?: return@mapNotNull null
            val name = connectivity.getLinkProperties(network)?.interfaceName ?: return@mapNotNull null
            InterfaceBindingCandidate(
                network,
                name,
                isBindablePhysicalNetwork(
                    capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN),
                    capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN),
                    capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_RESTRICTED),
                ),
            )
        }
    }

    fun interfaces(): List<BindableInterface> = candidates().filter { it.physical && it.interfaceName.isNotBlank() }
        .groupBy { it.interfaceName }.map { (name, matches) ->
            val network = matches.singleOrNull()?.network
            val link = network?.let { PolarisApplication.connectivity.getLinkProperties(it) }
            BindableInterface(name, name, link?.interfaceName == name,
                link?.linkAddresses?.mapNotNull { it.address.hostAddress }?.distinct()?.sorted().orEmpty())
        }.sortedWith(compareByDescending<BindableInterface> { it.isUp }.thenBy { it.displayName })

    fun bind(fd: Int, name: String, duplicateObserver: ((FileDescriptor) -> Unit)? = null) = bindInterfaceSocket(
        fd,
        name,
        ::candidates,
        { original -> ParcelFileDescriptor.fromFd(original).also { duplicateObserver?.invoke(it.fileDescriptor) } },
        { original -> PolarisVpnService.protectTransientSocket(original) },
    ) { network, duplicate ->
        network.bindSocket(duplicate.fileDescriptor)
    }
}

internal data class BindableInterface(val name: String, val displayName: String, val isUp: Boolean, val addresses: List<String>)
