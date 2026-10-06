import CryptoKit
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
    private var configContent: String?
    private var reportURL: URL?
    private let reportLock = NSLock()
    private let logger = Logger(subsystem: "com.polaris.app", category: "PacketTunnel")

    override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        let completion = TunnelCompletion<Error?>(completionHandler)
        do {
            guard let configuration = protocolConfiguration as? NETunnelProviderProtocol,
                  let identity = identity(from: configuration.providerConfiguration),
                  let supplied = options?["configContent"] as? String,
                  options?["sessionID"] as? String == identity.sessionID,
                  options?["requestID"] as? String == identity.requestID,
                  digest(supplied) == identity.configDigest
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
                    let base = try self.sharedDirectory()
                    let working = base.appendingPathComponent("tunnel", isDirectory: true)
                    let temp = working.appendingPathComponent("tmp", isDirectory: true)
                    try FileManager.default.createDirectory(at: temp, withIntermediateDirectories: true)
                    guard !supplied.isEmpty else { throw tunnelError("Tunnel configuration is empty") }
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
                    guard let server = LibboxNewStrictCommandServer(self.platform, self.platform, &error) else {
                        throw error ?? tunnelError("Libbox command server could not be created")
                    }
                    // Construction already owns OOM/power resources. Retain the handle
                    // before checking the error or Start so failure can close that owner.
                    self.commandServer = server
                    self.commandIdentity = identity
                    if let error { throw error }
                    try server.start()
                    guard self.lifecycle.accepts(token) else { throw tunnelError("Tunnel start was superseded") }
                    try server.startOrReloadService(supplied, options: LibboxOverrideOptions())
                    guard self.lifecycle.accepts(token) else { throw tunnelError("Tunnel start finished after cancellation/timeout") }
                    self.configContent = supplied
                    self.lifecycle.retainTailscaleStoreScope(server.exportTailscaleStoreRetirement(), identity: identity)
                    self.lifecycle.complete(token, phase: "running", stopped: false)
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
            }
            commandServer.close() // closes listener/OOM/power even if Start failed
        }
        commandServer = nil
        commandIdentity = nil
        configContent = nil
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
                guard self.lifecycle.accepts(token), let config = self.configContent,
                      let server = self.commandServer else { throw tunnelError("Tunnel reload was superseded") }
                self.reasserting = true
                defer { self.reasserting = false }
                try server.startOrReloadService(config, options: LibboxOverrideOptions())
                if let commandIdentity = self.commandIdentity {
                    self.lifecycle.retainTailscaleStoreScope(server.exportTailscaleStoreRetirement(), identity: commandIdentity)
                }
                guard self.lifecycle.accepts(token) else { throw tunnelError("Tunnel reload finished after cancellation/timeout") }
                self.lifecycle.complete(token, phase: "running", stopped: false)
                self.publishReport()
            } catch {
                self.lifecycle.complete(token, phase: "failed", stopped: nil, error: error.localizedDescription)
                self.publishReport()
                self.cancelTunnelWithError(error)
            }
        }
    }

    func settingsGeneration() -> UInt64 { lifecycle.currentGeneration() }
    func acceptsSettings(_ token: UInt64) -> Bool { lifecycle.accepts(token) }
    func settingsTimedOut(_ token: UInt64, error: Error) {
        lifecycle.settingsUncertain(token, error.localizedDescription)
        publishReport()
        cancelTunnelWithError(error)
    }

    func logDebugMessage(_ message: String) { logger.debug("\(message, privacy: .private)") }

    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)?) {
        if messageData != Data("status".utf8) {
            guard let object = try? JSONSerialization.jsonObject(with: messageData) as? [String: Any],
                  let command = object["command"] as? String,
                  let identity = identity(from: object) else { completionHandler?(nil); return }
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

    override func sleep(completionHandler: @escaping () -> Void) {
        let completion = TunnelCompletion<Void> { _ in completionHandler() }
        deadlines.asyncAfter(deadline: .now() + 2) { completion.finish(()) }
        worker.async { self.commandServer?.pause(); completion.finish(()) }
    }
    override func wake() { worker.async { self.commandServer?.wake() } }

    private func sharedDirectory() throws -> URL {
        guard let group = Bundle.main.object(forInfoDictionaryKey: "PolarisAppGroup") as? String,
              let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
        else { throw tunnelError("App Group is unavailable; sign both iOS targets with the same group") }
        return container.appendingPathComponent("polaris", isDirectory: true)
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
