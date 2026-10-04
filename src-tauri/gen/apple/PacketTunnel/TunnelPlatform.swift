import Foundation
import Libbox
import Network
import NetworkExtension

// Libbox is embedded only in the extension; UIKit/WebView stay in the host App.
final class TunnelPlatform: NSObject, LibboxPlatformInterfaceProtocol, LibboxCommandServerHandlerProtocol {
    private unowned let provider: PacketTunnelProvider
    private let monitorQueue = DispatchQueue(label: "com.polaris.tunnel-path")
    private var monitor: NWPathMonitor?
    private var settings: NEPacketTunnelNetworkSettings?
    private let stateLock = NSLock()
    private let settingsOperation = NSLock()

    init(provider: PacketTunnelProvider) { self.provider = provider }

    func openTun(_ options: LibboxTunOptionsProtocol?, ret0_: UnsafeMutablePointer<Int32>?) throws {
        guard let options, let ret0_ else { throw tunnelError("Missing TUN options or output") }
        let network = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: "127.0.0.1")
        network.mtu = NSNumber(value: options.getMTU())
        let ipv4 = prefixes(options.getInet4Address())
        if !ipv4.isEmpty {
            let ip = NEIPv4Settings(addresses: ipv4.map { $0.address() }, subnetMasks: ipv4.map { $0.mask() })
            let routes = prefixes(options.getInet4RouteAddress())
            ip.includedRoutes = options.getAutoRoute() ? (routes.isEmpty ? [.default()] : routes.map { NEIPv4Route(destinationAddress: $0.address(), subnetMask: $0.mask()) }) : []
            ip.excludedRoutes = prefixes(options.getInet4RouteExcludeAddress()).map { NEIPv4Route(destinationAddress: $0.address(), subnetMask: $0.mask()) }
            network.ipv4Settings = ip
        }
        let ipv6 = prefixes(options.getInet6Address())
        if !ipv6.isEmpty {
            let ip = NEIPv6Settings(addresses: ipv6.map { $0.address() }, networkPrefixLengths: ipv6.map { NSNumber(value: $0.prefix()) })
            let routes = prefixes(options.getInet6RouteAddress())
            ip.includedRoutes = options.getAutoRoute() ? (routes.isEmpty ? [.default()] : routes.map { NEIPv6Route(destinationAddress: $0.address(), networkPrefixLength: NSNumber(value: $0.prefix())) }) : []
            ip.excludedRoutes = prefixes(options.getInet6RouteExcludeAddress()).map { NEIPv6Route(destinationAddress: $0.address(), networkPrefixLength: NSNumber(value: $0.prefix())) }
            network.ipv6Settings = ip
        }
        if options.getAutoRoute(), options.getDNSMode()?.value != LibboxDNSModeDisabled {
            let servers = strings(try options.getDNSServerAddress())
            if !servers.isEmpty {
                let dns = NEDNSSettings(servers: servers)
                dns.matchDomains = [""]
                dns.matchDomainsNoSearch = true
                network.dnsSettings = dns
            }
        }
        if options.isHTTPProxyEnabled() {
            let proxy = NEProxySettings()
            let server = NEProxyServer(address: options.getHTTPProxyServer(), port: Int(options.getHTTPProxyServerPort()))
            proxy.httpServer = server
            proxy.httpsServer = server
            proxy.httpEnabled = true
            proxy.httpsEnabled = true
            proxy.exceptionList = strings(options.getHTTPProxyBypassDomain())
            proxy.matchDomains = strings(options.getHTTPProxyMatchDomain())
            network.proxySettings = proxy
        }
        guard settingsOperation.try() else { throw tunnelError("Another tunnel settings operation is already active") }
        defer { settingsOperation.unlock() }
        try apply(network)
        stateLock.lock(); settings = network; stateLock.unlock()
        // Upstream Libbox locates the extension's utun socket; no KVC/private key path.
        let fd = LibboxGetTunnelFileDescriptor()
        guard fd >= 0 else { throw tunnelError("The packet tunnel file descriptor is unavailable") }
        ret0_.pointee = fd
    }

    private func apply(_ network: NEPacketTunnelNetworkSettings?) throws {
        let token = provider.settingsGeneration()
        guard provider.acceptsSettings(token) else { throw tunnelError("Tunnel settings request is stale or uncertain") }
        let result = TunnelSettingsCompletion()
        provider.setTunnelNetworkSettings(network) { error in
            // A late callback may mean settings changed in the OS. It may not
            // commit readiness/configuration into a superseding extension request.
            guard self.provider.acceptsSettings(token) else {
                result.finish(tunnelError("Tunnel settings callback belongs to a superseded request")); return
            }
            result.finish(error)
        }
        do { try result.wait(seconds: 20) } catch {
            // Timeout cannot undo setTunnelNetworkSettings. Quarantine this
            // extension so no new start races a still-pending system callback.
            if error is TunnelSettingsTimeout { provider.settingsTimedOut(token, error: error) }
            throw error
        }
        guard provider.acceptsSettings(token) else { throw tunnelError("Tunnel settings completed after cancellation") }
    }

    func usePlatformAutoDetectControl() -> Bool { false }
    func autoDetectControl(_ fd: Int32) throws {}
    func bindInterfaceControl(_ fd: Int32, interfaceName: String?) throws {
        throw tunnelError("Binding sockets to a selected network interface is unavailable in the iOS packet tunnel")
    }
    func useProcFS() -> Bool { false }
    func underNetworkExtension() -> Bool { true }
    func includeAllNetworks() -> Bool { provider.protocolConfiguration.includeAllNetworks }
    func localDNSTransport() -> LibboxLocalDNSTransportProtocol? { nil }
    func readWIFIState() -> LibboxWIFIState? { nil }
    func registerMyInterface(_ name: String?) {}

    func startDefaultInterfaceMonitor(_ listener: LibboxInterfaceUpdateListenerProtocol?) throws {
        guard let listener else { throw tunnelError("Missing interface listener") }
        resetMonitor()
        let pathMonitor = NWPathMonitor()
        let token = provider.settingsGeneration()
        let first = DispatchSemaphore(value: 0)
        pathMonitor.pathUpdateHandler = { path in
            guard self.provider.acceptsSettings(token) else { first.signal(); return }
            let interface = path.status == .satisfied ? path.availableInterfaces.first : nil
            listener.updateDefaultInterface(interface?.name ?? "", interfaceIndex: interface.map { Int32($0.index) } ?? -1, isExpensive: path.isExpensive, isConstrained: path.isConstrained)
            listener.updateNetworkPath("\(path.status); ipv4=\(path.supportsIPv4); ipv6=\(path.supportsIPv6)")
            first.signal()
        }
        stateLock.lock(); monitor = pathMonitor; stateLock.unlock()
        pathMonitor.start(queue: monitorQueue)
        guard first.wait(timeout: .now() + 5) == .success else {
            pathMonitor.cancel()
            stateLock.lock()
            if monitor === pathMonitor { monitor = nil }
            stateLock.unlock()
            throw tunnelError("Reading the default network interface timed out")
        }
        guard provider.acceptsSettings(token) else {
            pathMonitor.cancel()
            throw tunnelError("Default interface observation was superseded")
        }
    }

    func closeDefaultInterfaceMonitor(_ listener: LibboxInterfaceUpdateListenerProtocol?) throws { resetMonitor() }
    func getInterfaces() throws -> LibboxNetworkInterfaceIteratorProtocol {
        stateLock.lock(); let monitor = self.monitor; stateLock.unlock()
        guard let monitor else { throw tunnelError("Interface monitor is not running") }
        return InterfaceIterator(monitor.currentPath.availableInterfaces.map { item in
            let result = LibboxNetworkInterface()
            result.name = item.name
            result.index = Int32(item.index)
            result.mtu = 1500
            result.flags = Int32(IFF_UP | IFF_RUNNING)
            switch item.type {
            case .wifi: result.type = LibboxInterfaceTypeWIFI
            case .cellular: result.type = LibboxInterfaceTypeCellular
            case .wiredEthernet: result.type = LibboxInterfaceTypeEthernet
            default: result.type = LibboxInterfaceTypeOther
            }
            return result
        })
    }
    private func resetMonitor() {
        stateLock.lock(); let previous = monitor; monitor = nil; stateLock.unlock()
        previous?.cancel()
    }
    func reset() { resetMonitor(); stateLock.lock(); settings = nil; stateLock.unlock() }
    func clearDNSCache() {
        guard settingsOperation.try() else {
            provider.cancelTunnelWithError(tunnelError("DNS settings change could not be accepted while another settings operation is active")); return
        }
        defer { settingsOperation.unlock() }
        stateLock.lock(); let settings = self.settings; stateLock.unlock()
        guard let settings else { return }
        do { try apply(nil); try apply(settings) } catch { provider.cancelTunnelWithError(error) }
    }

    func findConnectionOwner(_ ipProtocol: Int32, sourceAddress: String?, sourcePort: Int32, destinationAddress: String?, destinationPort: Int32) throws -> LibboxConnectionOwner { throw tunnelError("Process ownership is unavailable on iOS") }
    func startNeighborMonitor(_ listener: LibboxNeighborUpdateListenerProtocol?) throws { throw tunnelError("Neighbor monitoring is unavailable on iOS") }
    func closeNeighborMonitor(_ listener: LibboxNeighborUpdateListenerProtocol?) throws {}
    func usePlatformShell() -> Bool { false }
    func checkPlatformShell() throws { throw tunnelError("Shell sessions are unavailable on iOS") }
    func openShellSession(_ user: LibboxPlatformUser?, command: String?, environ: LibboxStringIteratorProtocol?, term: String?, rows: Int32, cols: Int32) throws -> LibboxShellSessionProtocol { throw tunnelError("Shell sessions are unavailable on iOS") }
    func lookupUser(_ username: String?) throws -> LibboxPlatformUser { throw tunnelError("User lookup is unavailable on iOS") }
    func lookupSFTPServer(_ error: NSErrorPointer) -> String { error?.pointee = tunnelError("SFTP server is unavailable on iOS"); return "" }
    func readSystemSSHHostKey(_ error: NSErrorPointer) -> String { error?.pointee = tunnelError("SSH host key is unavailable on iOS"); return "" }
    func tailscaleHostname() -> String { "Polaris-iOS" }
    func usePlatformBridge() -> Bool { false }
    func createBridge(_ options: LibboxBridgeOptions?) throws -> LibboxBridgeSessionProtocol { throw tunnelError("Network bridging is unavailable on iOS") }
    func usePlatformAutoRedirect() -> Bool { false }
    func createAutoRedirect(_ options: Data?, handler: LibboxAutoRedirectHandlerProtocol?) throws -> LibboxAutoRedirectSessionProtocol { throw tunnelError("Auto redirect is unavailable on iOS") }
    func send(_ notification: LibboxNotification?) throws { throw tunnelError("Extension notification delivery is not configured") }
    func cancelNotification(_ identifier: String?, typeID: Int32) throws {}

    func serviceStop() throws { provider.cancelTunnelWithError(nil) }
    func serviceReload() throws { try provider.enqueueReload() }
    func getSystemProxyStatus() throws -> LibboxSystemProxyStatus {
        stateLock.lock(); defer { stateLock.unlock() }
        let status = LibboxSystemProxyStatus()
        status.available = settings?.proxySettings?.httpServer != nil
        status.enabled = settings?.proxySettings?.httpEnabled ?? false
        return status
    }
    func setSystemProxyEnabled(_ enabled: Bool) throws {
        guard settingsOperation.try() else { throw tunnelError("Another tunnel settings operation is already active") }
        defer { settingsOperation.unlock() }
        stateLock.lock(); let settings = self.settings; stateLock.unlock()
        guard let settings, let proxy = settings.proxySettings else { throw tunnelError("Tunnel HTTP proxy is unavailable") }
        stateLock.lock()
        proxy.httpEnabled = enabled
        proxy.httpsEnabled = enabled
        stateLock.unlock()
        try apply(settings)
    }
    func triggerNativeCrash() throws { throw tunnelError("Native crash injection is disabled") }
    func writeDebugMessage(_ message: String?) { if let message { provider.logDebugMessage(message) } }
    func connectSSHAgent(_ ret0_: UnsafeMutablePointer<Int32>?) throws { throw tunnelError("SSH agent forwarding is unavailable on iOS") }
}

private final class InterfaceIterator: NSObject, LibboxNetworkInterfaceIteratorProtocol {
    private let values: [LibboxNetworkInterface]
    private var index = 0
    init(_ values: [LibboxNetworkInterface]) { self.values = values }
    func hasNext() -> Bool { index < values.count }
    func next() -> LibboxNetworkInterface? {
        guard hasNext() else { return nil }
        defer { index += 1 }
        return values[index]
    }
}
private func prefixes(_ iterator: LibboxRoutePrefixIteratorProtocol?) -> [LibboxRoutePrefix] {
    var result: [LibboxRoutePrefix] = []
    while iterator?.hasNext() == true { if let value = iterator?.next() { result.append(value) } }
    return result
}
private func strings(_ iterator: LibboxStringIteratorProtocol?) -> [String] {
    var result: [String] = []
    while iterator?.hasNext() == true { result.append(iterator?.next() ?? "") }
    return result
}
