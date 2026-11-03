import Darwin
import Foundation
import Libbox
import Network
import NetworkExtension

// Libbox is embedded only in the extension; UIKit/WebView stay in the host App.
final class TunnelPlatform: NSObject, LibboxPlatformInterfaceProtocol, LibboxCommandServerHandlerProtocol {
    fileprivate unowned let provider: PacketTunnelProvider
    private let monitorQueue = DispatchQueue(label: "com.polaris.tunnel-path")
    private let monitors = TunnelMonitorSession<NWPathMonitor, LibboxInterfaceUpdateListenerProtocol>()
    private var settings: NEPacketTunnelNetworkSettings?
    private var settingsReset = UUID()
    private let stateLock = NSLock()
    private let settingsOperation = NSLock()
    private var settingsCallbacks: [UUID: (UInt64, TunnelSettingsCompletion)] = [:]
    private var nativeTun = TunnelNativeTunOwner<NEPacketTunnelNetworkSettings>()

    init(provider: PacketTunnelProvider) { self.provider = provider }

    func openTun(_ options: LibboxTunOptionsProtocol?, ret0_: UnsafeMutablePointer<Int32>?) throws {
        guard let options, let ret0_ else { throw tunnelError("Missing TUN options or output") }
        guard !nativeTun.hasSessions else { throw tunnelError("Legacy TUN open cannot replace a native transaction owner") }
        let network = try networkSettings(options)
        guard settingsOperation.try() else { throw tunnelError("Another tunnel settings operation is already active") }
        defer { settingsOperation.unlock() }
        guard !nativeTun.hasSessions else { throw tunnelError("Legacy TUN open was superseded by a native transaction") }
        let token = provider.settingsGeneration()
        stateLock.lock(); let previous = settings; let reset = settingsReset; stateLock.unlock()
        try apply(network, token: token)
        try publish(network, replacing: previous, reset: reset, token: token)
        // Upstream Libbox locates the extension's utun socket; no KVC/private key path.
        let fd = LibboxGetTunnelFileDescriptor()
        guard fd >= 0 else { throw tunnelError("The packet tunnel file descriptor is unavailable") }
        ret0_.pointee = fd
    }

    private func networkSettings(_ options: LibboxTunOptionsProtocol) throws -> NEPacketTunnelNetworkSettings {
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
        return network
    }

    // Lock order: settingsOperation -> lifecycle publication gate -> stateLock.
    // Never wait for NE or invoke external code while either state lock is held.
    private func publish(_ network: NEPacketTunnelNetworkSettings,
                         replacing previous: NEPacketTunnelNetworkSettings?, reset: UUID, token: UInt64) throws {
        let accepted = provider.publishSettings(token) {
            self.stateLock.lock(); defer { self.stateLock.unlock() }
            // reset() must not be undone by an in-flight operation's callback.
            guard self.settingsReset == reset, self.settings === previous else { return false }
            self.settings = network
            return true
        }
        guard accepted else { throw tunnelError("Tunnel settings publication was superseded") }
        nativeTun.noteCurrentSettings(network)
    }

    private func apply(_ network: NEPacketTunnelNetworkSettings?, token: UInt64) throws {
        let owner = provider // Strongly retain the submitting provider until NE returns.
        guard owner.acceptsSettings(token) else { throw tunnelError("Tunnel settings request is stale or uncertain") }
        let requestID = UUID()
        let result = TunnelSettingsCompletion()
        stateLock.lock(); settingsCallbacks[requestID] = (token, result); stateLock.unlock()
        owner.setTunnelNetworkSettings(network) { [self, owner, network, result] error in
            // This is the actual callback, even if the waiter timed out or Stop
            // revoked its generation. Retain provider, candidate and result until
            // here; late settlement cannot publish or lift quarantine.
            withExtendedLifetime((owner, network, result)) {
                stateLock.lock(); settingsCallbacks.removeValue(forKey: requestID); stateLock.unlock()
                result.finish(error)
            }
        }
        do { try result.wait(seconds: 20) } catch {
            if error is TunnelSettingsTimeout { owner.settingsTimedOut(token, error: error) }
            throw error
        }
        guard owner.acceptsSettings(token) else { throw tunnelError("Tunnel settings completed after cancellation") }
    }

    func revokeNativeTun() { nativeTun.revoke() }

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
        let incarnation = nativeMonitorIdentity(listener)
        let pathMonitor = NWPathMonitor()
        let token = provider.settingsGeneration()
        guard provider.acceptsSettings(token) else { throw tunnelError("Default interface observation was superseded") }
        let publication = try monitors.publish(incarnation, monitor: pathMonitor, listener: listener, generation: token)
        guard publication.inserted else { return }
        cancelMonitor(publication.previous)
        let record = publication.record
        pathMonitor.pathUpdateHandler = { [weak self, weak record] path in
            guard let self, let record else { return }
            guard self.provider.acceptsSettings(record.generation) else {
                // Revoke this incarnation only; an old path handler cannot cancel its successor.
                self.cancelMonitor(try? self.monitors.close(String(record.incarnation)))
                return
            }
            guard let target = self.monitors.beginDelivery(record) else { return }
            var observed = false
            defer { self.monitors.finishDelivery(record, observed: observed) }
            let interface = path.status == .satisfied ? path.availableInterfaces.first : nil
            // The local target belongs to the admitted record even if Close/Reload races Go.
            target.updateDefaultInterface(interface?.name ?? "", interfaceIndex: interface.map { Int32($0.index) } ?? -1, isExpensive: path.isExpensive, isConstrained: path.isConstrained)
            target.updateNetworkPath("\(path.status); ipv4=\(path.supportsIPv4); ipv6=\(path.supportsIPv6)")
            observed = self.provider.acceptsSettings(record.generation)
        }
        guard monitors.beginRegistration(record) else {
            throw tunnelError("Default interface observation was superseded before SDK start")
        }
        pathMonitor.start(queue: monitorQueue)
        monitors.finishRegistration(record)?.cancel()
        guard record.first.wait(timeout: .now() + 5) == .success else {
            cancelMonitor(try monitors.close(incarnation))
            throw tunnelError("Reading the default network interface timed out")
        }
        guard provider.acceptsSettings(token), monitors.hasObservation(record) else {
            cancelMonitor(try monitors.close(incarnation))
            throw tunnelError("Default interface observation was superseded")
        }
    }

    func closeDefaultInterfaceMonitor(_ listener: LibboxInterfaceUpdateListenerProtocol?) throws {
        guard let listener else { throw tunnelError("Missing interface listener") }
        cancelMonitor(try monitors.close(nativeMonitorIdentity(listener)))
    }
    func getInterfaces() throws -> LibboxNetworkInterfaceIteratorProtocol {
        guard let monitor = monitors.current()?.monitor else { throw tunnelError("Interface monitor is not running") }
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
        cancelMonitor(monitors.reset())
    }
    private func nativeMonitorIdentity(_ listener: LibboxInterfaceUpdateListenerProtocol) -> String {
        let value: String? = LibboxInterfaceUpdateListenerIdentity(listener)
        return value ?? "" // Empty/foreign/saturated native identities are rejected by the gate.
    }
    private func cancelMonitor(_ record: TunnelMonitorSession<NWPathMonitor, LibboxInterfaceUpdateListenerProtocol>.Record?) {
        if let record { monitors.takeCancellation(record)?.cancel() }
    }
    func reset() -> String? {
        resetMonitor()
        stateLock.lock()
        let pending = settingsCallbacks.values.first
        if pending == nil && !nativeTun.hasSessions { settingsReset = UUID(); settings = nil }
        stateLock.unlock()
        if let pending {
            let message = "The original NE settings callback has not joined"
            provider.nativeTunCleanupUncertain(pending.0, message: message)
            return message
        }
        guard !nativeTun.hasSessions else { return "Native TUN settings or descriptor custody remains uncertain" }
        nativeTun = TunnelNativeTunOwner<NEPacketTunnelNetworkSettings>()
        return nil
    }
    func clearDNSCache() {
        guard settingsOperation.try() else {
            provider.cancelTunnelWithError(tunnelError("DNS settings change could not be accepted while another settings operation is active")); return
        }
        defer { settingsOperation.unlock() }
        guard !nativeTun.blocksSettingsChanges else {
            provider.cancelTunnelWithError(tunnelError("Native TUN transaction holds the DNS settings lease")); return
        }
        let token = provider.settingsGeneration()
        stateLock.lock(); let settings = self.settings; let reset = settingsReset; stateLock.unlock()
        guard let settings else { return }
        do {
            try apply(nil, token: token)
            stateLock.lock(); let retained = settingsReset == reset && self.settings === settings; stateLock.unlock()
            guard retained else { throw tunnelError("DNS settings replay was superseded by reset") }
            // Both calls belong to one operation; never borrow a newer generation.
            try apply(settings, token: token)
        } catch { provider.cancelTunnelWithError(error) }
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
        guard nativeTun.settingsArePublished, provider.acceptsSettings(provider.settingsGeneration()) else {
            throw tunnelError("Tunnel proxy settings are not published by the current owner")
        }
        stateLock.lock(); defer { stateLock.unlock() }
        let status = LibboxSystemProxyStatus()
        status.available = settings?.proxySettings?.httpServer != nil
        status.enabled = settings?.proxySettings?.httpEnabled ?? false
        return status
    }
    func setSystemProxyEnabled(_ enabled: Bool) throws {
        guard settingsOperation.try() else { throw tunnelError("Another tunnel settings operation is already active") }
        defer { settingsOperation.unlock() }
        guard !nativeTun.blocksSettingsChanges else { throw tunnelError("Native TUN transaction holds the proxy settings lease") }
        let token = provider.settingsGeneration()
        stateLock.lock(); let settings = self.settings; let reset = settingsReset; stateLock.unlock()
        guard let settings, let retainedProxy = settings.proxySettings else { throw tunnelError("Tunnel HTTP proxy is unavailable") }
        // NE settings are reference types. Copy the nested object explicitly:
        // a shallow network copy alone could expose the pending toggle to readers.
        guard let candidate = settings.copy() as? NEPacketTunnelNetworkSettings,
              let proxy = retainedProxy.copy() as? NEProxySettings else {
            throw tunnelError("Tunnel HTTP proxy settings could not be copied")
        }
        proxy.httpEnabled = enabled
        proxy.httpsEnabled = enabled
        candidate.proxySettings = proxy
        try apply(candidate, token: token)
        try publish(candidate, replacing: settings, reset: reset, token: token)
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


#if POLARIS_NATIVE_TUN_V1
// Defined only when the selected framework's actual generated header exposes
// the complete V1 protocol. Old pinned headers compile the legacy path above.
extension TunnelPlatform: LibboxRuntimeTunPlatformProtocol {
    func runtimeTunVersion() -> Int32 { 1 }

    func newTunSession(_ options: LibboxTunOptionsProtocol?, previous: LibboxRuntimeTunSessionProtocol?,
                       receipt: LibboxRuntimeTunReceipt?) throws -> LibboxRuntimeTunSessionProtocol {
        guard let options, let receipt else { throw tunnelError("Native TUN receipt or options are absent") }
        let suppliedOwnerID: String? = receipt.ownerID()
        guard let ownerID = suppliedOwnerID, !ownerID.isEmpty else { throw tunnelError("Native TUN receipt identity is absent") }
        guard settingsOperation.try() else { throw tunnelError("Tunnel settings operation is already active") }
        defer { settingsOperation.unlock() }
        let token = provider.settingsGeneration()
        guard provider.acceptsSettings(token) else { throw tunnelError("Native TUN provider was revoked") }
        let old = previous as? TunnelNativeTunSession
        guard previous == nil || (old != nil && old!.platform === self && old!.owner === nativeTun) else {
            throw tunnelError("Native TUN previous session belongs to another provider")
        }
        let network = try networkSettings(options)
        stateLock.lock(); let snapshot = old == nil ? nil : settings; stateLock.unlock()
        let state = try nativeTun.allocate(ownerID, token: token, network: network, snapshot: snapshot,
                                          previous: old?.state, published: { receipt.published() })
        return TunnelNativeTunSession(platform: self, owner: nativeTun, state: state, receipt: receipt)
    }

    fileprivate func nativeTunApply(_ network: NEPacketTunnelNetworkSettings?, token: UInt64) throws {
        guard settingsOperation.try() else { throw tunnelError("Another tunnel settings operation is active") }
        defer { settingsOperation.unlock() }
        try apply(network, token: token)
    }
    fileprivate func nativeTunCache(_ network: NEPacketTunnelNetworkSettings?) {
        stateLock.lock(); settings = network; stateLock.unlock()
    }
    fileprivate func nativeTunAcquireFile() throws -> TunnelNativeTunFile {
        let original = LibboxGetTunnelFileDescriptor()
        guard original >= 0 else { throw tunnelError("Provider utun descriptor is unavailable") }
        let custody = Darwin.dup(original)
        guard custody >= 0 else { throw tunnelError("Could not retain native utun descriptor custody") }
        // The held duplicate pins this exact socket's lifetime. The fd integer
        // never serves as a provider, operation, settings or receipt identity.
        return TunnelNativeTunFile(custody, check: {
            guard Darwin.fcntl(custody, F_GETFD) >= 0 else { throw tunnelError("Native utun custody was lost") }
        }, dispose: {
            guard Darwin.close(custody) == 0 else { throw tunnelError("Native utun custody close is unconfirmed") }
        })
    }
}

private final class TunnelNativeTunSession: NSObject, LibboxRuntimeTunSessionProtocol {
    fileprivate let platform: TunnelPlatform
    fileprivate let owner: TunnelNativeTunOwner<NEPacketTunnelNetworkSettings>
    fileprivate let state: TunnelNativeTunOwner<NEPacketTunnelNetworkSettings>.Session
    private let receipt: LibboxRuntimeTunReceipt
    private let provider: PacketTunnelProvider // Exact original provider lives as long as this session.
    init(platform: TunnelPlatform, owner: TunnelNativeTunOwner<NEPacketTunnelNetworkSettings>,
         state: TunnelNativeTunOwner<NEPacketTunnelNetworkSettings>.Session, receipt: LibboxRuntimeTunReceipt) {
        self.platform = platform; self.owner = owner; self.state = state; self.receipt = receipt
        self.provider = platform.provider
    }
    func ownerID() -> String { state.ownerID }
    func prepare() throws {
        try owner.prepare(state, accepts: provider.acceptsSettings,
                          apply: platform.nativeTunApply, acquire: platform.nativeTunAcquireFile,
                          cache: platform.nativeTunCache)
    }
    func fileDescriptor() -> Int32 { owner.descriptor(state, accepts: provider.acceptsSettings) }
    func validate() throws { try owner.validate(state, accepts: provider.acceptsSettings) }
    func abort() throws {
        try owner.abort(state, accepts: provider.acceptsSettings,
                        apply: platform.nativeTunApply, acquire: platform.nativeTunAcquireFile,
                        cache: platform.nativeTunCache)
    }
    func retire() throws {
        try owner.retire(state, accepts: provider.acceptsSettings,
                         apply: platform.nativeTunApply, cache: platform.nativeTunCache)
    }
}
#endif
