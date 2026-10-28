import CryptoKit
import Darwin
import Foundation
import Libbox
import NetworkExtension
import os

final class PacketTunnelProvider: NEPacketTunnelProvider {
    private let worker = DispatchQueue(label: "com.polaris.packet-tunnel")
    private let deadlines = DispatchQueue(label: "com.polaris.packet-tunnel-deadlines")
    private let lifecycle = TunnelLifecycle()
    private lazy var platform = TunnelPlatform(provider: self)
    // These handles and config bytes are only accessed on worker.
    private var commandServer: LibboxCommandServer?
    private var commandIdentity: TunnelIdentity?
    private var runtimeRestartOwner: TunnelRuntimeRestartOwner?
    private var managedBootstrap: (identity: TunnelIdentity, token: UInt64, ownerNonce: String, completion: TunnelCompletion<Error?>)?
    private var reportURL: URL?
    private let reportLock = NSLock()
    private let logger = Logger(subsystem: "com.polaris.app", category: "PacketTunnel")

    override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        let completion = TunnelCompletion<Error?>(completionHandler)
        do {
            let bootstrap = options?["directoryBootstrapVersion"] as? String == "polaris-ios-g4-bootstrap-v1"
            let supplied = options?["configContent"] as? String
            guard let configuration = protocolConfiguration as? NETunnelProviderProtocol,
                  let identity = identity(from: configuration.providerConfiguration),
                  options?["sessionID"] as? String == identity.sessionID,
                  options?["requestID"] as? String == identity.requestID,
                  bootstrap ? supplied == nil : (supplied.map { digest($0) == identity.configDigest } ?? false)
            else { throw tunnelError("The tunnel start request does not match its saved session/configuration identity") }
            try prepareReportURL()
            let token = try lifecycle.start(identity)
            publishReport()
            deadlines.asyncAfter(deadline: .now() + 35) {
                if completion.finish(tunnelError("Tunnel start timed out; runtime and cleanup remain uncertain"), before: {
                    self.lifecycle.timeout(token, "Tunnel start timed out; runtime and cleanup remain uncertain")
                    self.publishReport()
                }) {
                    self.cancelTunnelWithError(tunnelError("Tunnel start timed out"))
                }
            }
            worker.async {
                do {
                    guard self.lifecycle.accepts(token) else { throw tunnelError("Tunnel start was superseded") }
                    let base = try self.sharedDirectory().resolvingSymlinksInPath()
                    let working = base.appendingPathComponent("tunnel", isDirectory: true)
                    let temp = working.appendingPathComponent("tmp", isDirectory: true)
                    try FileManager.default.createDirectory(at: temp, withIntermediateDirectories: true)
                    guard bootstrap || supplied?.isEmpty == false else { throw tunnelError("Tunnel configuration is empty") }
                    let setup = LibboxSetupOptions()
                    setup.basePath = base.path
                    setup.workingPath = working.path
                    setup.tempPath = temp.path
                    setup.logMaxLines = 500
                    setup.oomKillerEnabled = true
                    setup.crashReportSource = "PolarisPacketTunnel"
                    var error: NSError?
                    LibboxSetup(setup, &error)
                    if let error { throw error }
                    let ownerNonce = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
                    let constructed = bootstrap
                        ? LibboxNewManagedStrictCommandServer(self.platform, self.platform, base.path, ownerNonce, &error)
                        : LibboxNewStrictCommandServer(self.platform, self.platform, &error)
                    guard let server = constructed else {
                        throw error ?? tunnelError("Libbox command server could not be created")
                    }
                    // Construction already owns OOM/power resources. Retain the handle
                    // before checking the error or Start so failure can close that owner.
                    self.commandServer = server
                    self.commandIdentity = identity
                    if let error { throw error }
                    try server.start()
                    guard self.lifecycle.accepts(token) else { throw tunnelError("Tunnel start was superseded") }
                    if bootstrap {
                        // No service/run/config exists yet. Keep Start pending until
                        // the host verifies this exact server's ABI/source and activates.
                        self.managedBootstrap = (identity, token, ownerNonce, completion)
                        return
                    }
                    guard let supplied else { throw tunnelError("Tunnel configuration is absent") }
                    try self.startRuntimeOwnedService(server, config: supplied, token: token)
                    guard self.lifecycle.accepts(token) else { throw tunnelError("Tunnel start finished after cancellation/timeout") }
                    self.lifecycle.retainTailscaleStoreScope(server.exportTailscaleStoreRetirement(), identity: identity)
                    self.lifecycle.retainTailscaleDirectoryScope(server.exportTailscaleDirectoryRetirement(), identity: identity)
                    guard self.lifecycle.complete(token, phase: "running", stopped: false) else {
                        throw tunnelError("Tunnel ready publication was superseded")
                    }
                    self.publishReport()
                    completion.finish(nil)
                } catch {
                    let cleanup = self.closeCore()
                    self.lifecycle.complete(token, phase: "failed", stopped: true,
                                            error: error.localizedDescription, cleanup: cleanup)
                    self.publishReport()
                    self.logger.error("Tunnel start failed: \(error.localizedDescription, privacy: .public)")
                    completion.finish(error)
                }
            }
        } catch { completion.finish(error) }
    }

    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        let completion = TunnelCompletion<Void> { _ in completionHandler() }
        guard let token = lifecycle.stop() else { completion.finish(()); return }
        publishReport()
        deadlines.asyncAfter(deadline: .now() + 15) {
            completion.finish((), before: {
                self.lifecycle.timeout(token, "Tunnel stop timed out; Go worker has not confirmed completion")
                self.publishReport()
            })
        }
        worker.async {
            let cleanup = self.closeCore()
            self.lifecycle.complete(token, phase: "stopped", stopped: true, cleanup: cleanup)
            // Publish before NE tells the host that the system tunnel is disconnected.
            self.publishReport()
            completion.finish(())
        }
    }

    /// Close only the explicitly held CommandServer. Its ordinary CloseService failure
    /// remains sticky and visible even after Go detaches its Instance reference.
    private func closeCore() -> String? {
        var failure: String?
        if let pending = managedBootstrap {
            managedBootstrap = nil
            pending.completion.finish(tunnelError("Managed bootstrap was stopped before activation"))
        }
        runtimeRestartOwner?.revoke()
        if let commandServer {
            do { try commandServer.closeService() } catch {
                failure = error.localizedDescription
                lifecycle.retainCleanupFailure(error.localizedDescription)
                logger.error("Core close failed: \(error.localizedDescription, privacy: .public)")
            }
            // The Go export preserves all original runs, including failed/reloaded
            // runs. Collect its immutable terminal before losing this exact handle.
            if let commandIdentity {
                lifecycle.retainTailscaleStoreScope(commandServer.exportTailscaleStoreRetirement(), identity: commandIdentity)
                lifecycle.retainTailscaleDirectoryScope(commandServer.exportTailscaleDirectoryRetirement(), identity: commandIdentity, terminal: true)
            }
            commandServer.close() // closes listener/OOM/power even if Start failed
        }
        commandServer = nil
        commandIdentity = nil
        runtimeRestartOwner = nil
        platform.reset()
        return failure
    }

    /// Called by Go's RPC handler. Waiting here would deadlock against Go's service
    /// lock when worker calls back into StartOrReloadService. Success means accepted.
    func enqueueReload() throws {
        let token = try lifecycle.reload()
        publishReport()
        deadlines.asyncAfter(deadline: .now() + 35) {
            if self.lifecycle.timeout(token, "Accepted reload timed out; completion is unknown", expectedPhase: "reloadAccepted") {
                self.publishReport()
                self.cancelTunnelWithError(tunnelError("Tunnel reload timed out"))
            }
        }
        worker.async {
            do {
                guard self.lifecycle.accepts(token), let config = self.runtimeRestartOwner?.snapshot(),
                      let server = self.commandServer else { throw tunnelError("Tunnel reload was superseded") }
                self.reasserting = true
                defer { self.reasserting = false }
                try self.startRuntimeOwnedService(server, config: config, token: token)
                if let commandIdentity = self.commandIdentity {
                    self.lifecycle.retainTailscaleStoreScope(server.exportTailscaleStoreRetirement(), identity: commandIdentity)
                    self.lifecycle.retainTailscaleDirectoryScope(server.exportTailscaleDirectoryRetirement(), identity: commandIdentity)
                }
                guard self.lifecycle.accepts(token) else { throw tunnelError("Tunnel reload finished after cancellation/timeout") }
                guard self.lifecycle.complete(token, phase: "running", stopped: false) else {
                    throw tunnelError("Tunnel reload ready publication was superseded")
                }
                self.publishReport()
            } catch {
                // Stop/timeout may already own the generation. Never cancel a successor.
                if self.lifecycle.complete(token, phase: "failed", stopped: nil, error: error.localizedDescription) {
                    self.publishReport()
                    self.cancelTunnelWithError(error)
                }
            }
        }
    }

    func settingsGeneration() -> UInt64 { lifecycle.currentGeneration() }
    func acceptsSettings(_ token: UInt64) -> Bool { lifecycle.accepts(token) }
    func publishSettings(_ token: UInt64, publication: () -> Bool) -> Bool {
        lifecycle.publishSettings(token, publication: publication)
    }
    func settingsTimedOut(_ token: UInt64, error: Error) {
        lifecycle.settingsUncertain(token, error.localizedDescription)
        publishReport()
        cancelTunnelWithError(error)
    }

    func logDebugMessage(_ message: String) { logger.debug("\(message, privacy: .private)") }

    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)?) {
        if messageData == Data("tls-fingerprints".utf8) {
            // Optional symbol permits old static libbox. Hash the containing
            // executable, never relabel an input archive hash as loaded bytes.
            guard let executable = Bundle.main.executableURL else { completionHandler?(nil); return }
            worker.async {
                do {
                    func digest() throws -> String {
                        let file = try FileHandle(forReadingFrom: executable)
                        defer { try? file.close() }
                        var hash = SHA256()
                        while let bytes = try file.read(upToCount: 65536), !bytes.isEmpty { hash.update(data: bytes) }
                        return hash.finalize().map { String(format: "%02x", $0) }.joined()
                    }
                    let before = try digest()
                    var document: String?
                    if let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "LibboxTLSFingerprintCapabilities") {
                        // dlsym may resolve a same-named export in a different image.
                        // Bind this query to the final extension container; this is not
                        // a claim to hash relocated in-memory executable pages.
                        var image = Dl_info()
                        guard dladdr(symbol, &image) != 0, let name = image.dli_fname,
                              tlsCapabilityImageMatches(String(cString: name), executable: executable)
                        else { completionHandler?(nil); return }
                        typealias Query = @convention(c) (AutoreleasingUnsafeMutablePointer<NSError?>?) -> NSString?
                        let query = unsafeBitCast(symbol, to: Query.self)
                        var error: NSError?
                        document = query(&error) as String?
                        if error != nil { document = nil }
                    }
                    guard (document?.utf8.count ?? 0) <= 65536, before == (try digest())
                    else { completionHandler?(nil); return }
                    let reply: [String: Any] = ["document": document as Any? ?? NSNull(), "binarySha256": before]
                    completionHandler?(try JSONSerialization.data(withJSONObject: reply))
                } catch { completionHandler?(nil) }
            }
            return
        }
        if messageData != Data("status".utf8) {
            guard let object = try? JSONSerialization.jsonObject(with: messageData) as? [String: Any],
                  let command = object["command"] as? String,
                  let identity = identity(from: object) else { completionHandler?(nil); return }
            if command == "bootstrapCapability" || command == "activateManaged" {
                worker.async { self.handleManagedBootstrap(command, object: object, identity: identity, reply: completionHandler) }
                return
            }
            if command == "observeSession" {
                guard let nonce = object["observationNonce"] as? String,
                      let observation = lifecycle.observeSession(identity: identity, nonce: nonce,
                          expectedGeneration: (object["extensionGeneration"] as? NSNumber)?.uint64Value)
                else { completionHandler?(nil); return }
                completionHandler?(try? JSONEncoder().encode(observation))
                return
            }
            guard command == "prepareStop", let requestID = object["stopRequestID"] as? String else { completionHandler?(nil); return }
            if let nonce = object["observationNonce"] as? String {
                guard lifecycle.prepareStop(identity: identity, requestID: requestID, nonce: nonce,
                    expectedGeneration: (object["extensionGeneration"] as? NSNumber)?.uint64Value,
                    observation: { observation in completionHandler?(try? JSONEncoder().encode(observation)) })
                else { completionHandler?(nil); return }
                publishReport()
                return // Completed by the actual normal Stop worker, never this ACK.
            }
            guard lifecycle.prepareStop(identity: identity, requestID: requestID) else { completionHandler?(nil); return }
            publishReport()
        }
        // The Go worker may be blocked; status is a lock-protected observation.
        completionHandler?(try? JSONEncoder().encode(lifecycle.report()))
    }

    /// Runs only on the original worker, which also owns Start/Stop/CommandServer.
    private func handleManagedBootstrap(_ command: String, object: [String: Any], identity: TunnelIdentity,
                                        reply: ((Data?) -> Void)?) {
        guard let pending = managedBootstrap, pending.identity == identity, commandIdentity == identity,
              lifecycle.accepts(pending.token), let server = commandServer,
              let nonce = object["observationNonce"] as? String, !nonce.isEmpty,
              let capability = try? JSONSerialization.jsonObject(with: Data(server.exportTailscaleDirectoryCapability().utf8)) as? [String: Any],
              capability["ownerNonce"] as? String == pending.ownerNonce else { reply?(nil); return }
        if command == "bootstrapCapability" {
            let response: [String: Any] = ["contractVersion": "polaris-ios-g4-bootstrap-v1",
                "identity": ["sessionID": identity.sessionID, "requestID": identity.requestID, "configDigest": identity.configDigest],
                "extensionGeneration": pending.token, "observationNonce": nonce, "capability": capability]
            reply?(try? JSONSerialization.data(withJSONObject: response)); return
        }
        guard object["ownerNonce"] as? String == pending.ownerNonce,
              (object["extensionGeneration"] as? NSNumber)?.uint64Value == pending.token,
              let expectedSource = object["sourceIdentity"] as? String,
              capability["sourceIdentity"] as? String == expectedSource,
              capability["participating"] as? Bool == true,
              let config = object["configContent"] as? String, !config.isEmpty,
              digest(config) == identity.configDigest else { reply?(nil); return }
        // One-shot, before crossing Go: neither a replay nor a late callback may
        // activate another run under the original bootstrap approval.
        managedBootstrap = nil
        do {
            try server.activateTailscaleDirectoryDomain(pending.ownerNonce, expectedSourceIdentity: expectedSource)
            guard lifecycle.accepts(pending.token) else { throw tunnelError("Managed activation was superseded") }
            try startRuntimeOwnedService(server, config: config, token: pending.token)
            guard lifecycle.accepts(pending.token) else { throw tunnelError("Managed activation returned after timeout/Stop") }
            lifecycle.retainTailscaleStoreScope(server.exportTailscaleStoreRetirement(), identity: identity)
            lifecycle.retainTailscaleDirectoryScope(server.exportTailscaleDirectoryRetirement(), identity: identity)
            guard lifecycle.complete(pending.token, phase: "running", stopped: false),
                  let observation = lifecycle.observeSession(identity: identity, nonce: nonce, expectedGeneration: pending.token) else {
                throw tunnelError("Managed activation lost its original generation")
            }
            publishReport()
            // Real tunnel settings were applied by StartOrReloadService. Only now
            // may NE and the host observe successful original Start completion.
            pending.completion.finish(nil)
            reply?(try? JSONEncoder().encode(observation))
        } catch {
            let cleanup = closeCore()
            lifecycle.complete(pending.token, phase: "failed", stopped: true,
                error: error.localizedDescription, cleanup: cleanup)
            publishReport()
            pending.completion.finish(error)
            reply?(nil)
        }
    }

    override func sleep(completionHandler: @escaping () -> Void) {
        let completion = TunnelCompletion<Void> { _ in completionHandler() }
        deadlines.asyncAfter(deadline: .now() + 2) { completion.finish(()) }
        worker.async {
            self.commandServer?.recordScreenState(false)
            self.commandServer?.pause()
            completion.finish(())
        }
    }
    override func wake() {
        worker.async {
            self.commandServer?.recordScreenState(true)
            // Go Wake() only clears the pause on Android. iOS needs WakeNow().
            self.commandServer?.wakeNow()
        }
    }

    /// One callback owner per native instance. Never dispatch-sync a Go callback
    /// onto worker: Close on worker joins that same persistence callback.
    private func startRuntimeOwnedService(_ server: LibboxCommandServer, config: String, token: UInt64) throws {
        runtimeRestartOwner?.revoke()
        let owner = TunnelRuntimeRestartOwner(config: config, lifecycle: lifecycle, token: token)
        runtimeRestartOwner = owner
        do {
            try server.startOrReloadService(withRetryableRuntimePersistence: config, options: LibboxOverrideOptions(), handler: owner)
            guard lifecycle.accepts(token), commandServer === server, runtimeRestartOwner === owner else {
                throw tunnelError("Runtime protocol query lost its native owner")
            }
            // Go (string, error) follows the generated nonnull NSString + NSErrorPointer
            // convention used by readSystemSSHHostKey. Exact runtimeCapabilities header
            // and Swift import must also be checked in the matching component build.
            var capabilityError: NSError?
            let document = server.runtimeCapabilities(&capabilityError)
            if let capabilityError { throw capabilityError }
            let platformBuild: String
            #if arch(arm64)
            platformBuild = "ios/arm64"
            #else
            throw tunnelError("Unsupported iOS runtime build target")
            #endif
            let admission = try TunnelRuntimeAdmission.decode(document, platform: platformBuild)
            guard lifecycle.accepts(token), commandServer === server, runtimeRestartOwner === owner else {
                throw tunnelError("Runtime protocol response lost its native owner")
            }
            try owner.bindRuntimeAdmission(admission)
            guard lifecycle.admitRuntimeProtocol(token, admission: admission) else {
                throw tunnelError("Runtime protocol admission was superseded")
            }
        } catch {
            owner.revoke()
            throw error
        }
    }

    private func sharedDirectory() throws -> URL {
        guard let group = Bundle.main.object(forInfoDictionaryKey: "PolarisAppGroup") as? String,
              let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
        else { throw tunnelError("App Group is unavailable; sign both iOS targets with the same group") }
        let directory = container.appendingPathComponent("polaris", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory.resolvingSymlinksInPath()
    }
    private func prepareReportURL() throws {
        let directory = try sharedDirectory()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        reportLock.lock(); reportURL = directory.appendingPathComponent("tunnel-lifecycle.json"); reportLock.unlock()
    }
    private func publishReport() {
        reportLock.lock(); defer { reportLock.unlock() }
        guard let reportURL else { return }
        do {
            try JSONEncoder().encode(lifecycle.report()).write(to: reportURL,
                options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        } catch { logger.error("Tunnel lifecycle report could not be saved: \(error.localizedDescription, privacy: .public)") }
    }
}

private func identity(from values: [String: Any]?) -> TunnelIdentity? {
    guard let sessionID = values?["sessionID"] as? String,
          let requestID = values?["requestID"] as? String,
          let configDigest = values?["configDigest"] as? String,
          !sessionID.isEmpty, !requestID.isEmpty, !configDigest.isEmpty else { return nil }
    return TunnelIdentity(sessionID: sessionID, requestID: requestID, configDigest: configDigest)
}
private func digest(_ config: String) -> String {
    SHA256.hash(data: Data(config.utf8)).map { String(format: "%02x", $0) }.joined()
}
func tunnelError(_ message: String) -> NSError {
    NSError(domain: "PolarisPacketTunnel", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
}

/// This acknowledges the current provider's reload source only. The kernel keeps
/// regeneration persistence pending until Rust separately proves authoritative D.
/// No shared-profile file or byte-exact crash-replay guarantee is introduced.
private final class TunnelRuntimeRestartOwner: NSObject, LibboxRuntimePersistenceHandlerProtocol {
    private let lock = NSLock()
    private let lifecycle: TunnelLifecycle
    private let token: UInt64
    private var content: String
    private var revoked = false
    private var incarnation: String?
    private var admittedIncarnation: String?
    private var revision: Int64 = -1
    private var effectiveDigest: String?

    init(config: String, lifecycle: TunnelLifecycle, token: UInt64) {
        self.content = config
        self.lifecycle = lifecycle
        self.token = token
    }

    func runtimePersistenceMode() -> String { "app_authoritative_regeneration" }

    func snapshot() -> String? {
        lock.lock(); defer { lock.unlock() }
        return revoked ? nil : content
    }

    func revoke() { lock.lock(); revoked = true; lock.unlock() }

    func bindRuntimeAdmission(_ admission: TunnelRuntimeAdmission) throws {
        lock.lock(); defer { lock.unlock() }
        guard !revoked, lifecycle.accepts(token) else { throw tunnelError("Runtime admission owner was superseded") }
        admittedIncarnation = admission.identity?.incarnation
    }

    func persistRuntimeConfiguration(_ incarnation: String?, revision: Int64,
                                     effectiveDigest: String?, configContent: String?) throws {
        guard let incarnation, !incarnation.isEmpty, revision >= 0,
              let effectiveDigest, let configContent, !configContent.isEmpty,
              digest(configContent) == effectiveDigest else {
            throw tunnelError("Runtime reload receipt identity is invalid")
        }
        lock.lock(); defer { lock.unlock() }
        guard !revoked, self.incarnation == nil || self.incarnation == incarnation,
              admittedIncarnation == incarnation, revision >= self.revision,
              revision != self.revision || self.effectiveDigest == effectiveDigest else {
            throw tunnelError("Runtime reload receipt belongs to a stale owner")
        }
        guard lifecycle.publishSettings(token, publication: {
            self.incarnation = incarnation
            self.revision = revision
            self.effectiveDigest = effectiveDigest
            self.content = configContent
            return true
        }) else { throw tunnelError("Runtime reload owner was superseded") }
    }
}
