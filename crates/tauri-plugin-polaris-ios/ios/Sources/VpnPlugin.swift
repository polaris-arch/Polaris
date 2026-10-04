import CryptoKit
import Foundation
import NetworkExtension
import Tauri
import WebKit

private struct StartArgs: Decodable { let configContent: String; let requestID: String }
private struct StopArgs: Decodable { let requestID: String }
private struct LoadedVpnManager {
    let manager: NETunnelProviderManager?
    let existing: Bool
}

private final class VpnOperation {
    let invoke: Invoke
    let generation: UInt64
    let kind: String
    let requestID: String
    var manager: NETunnelProviderManager?
    var started = false
    var observer: NSObjectProtocol?
    var deadline: DispatchWorkItem?
    init(_ invoke: Invoke, generation: UInt64, kind: String, requestID: String) {
        self.invoke = invoke; self.generation = generation; self.kind = kind; self.requestID = requestID
    }
}

final class VpnPlugin: Plugin {
    private let viewportObservers = NSMapTable<WKWebView, ViewportObserver>(keyOptions: .weakMemory, valueOptions: .strongMemory)

    override func load(webview: WKWebView) {
        super.load(webview: webview)
        DispatchQueue.main.async { [weak self, weak webview] in
            guard let self, let webview else { return }
            if let observer = self.viewportObservers.object(forKey: webview), observer.observes(webview) {
                observer.refresh(force: true)
                return
            }
            let observer = ViewportObserver(webview: webview)
            self.viewportObservers.setObject(observer, forKey: webview)
            webview.addSubview(observer)
            observer.refresh(force: true)
        }
    }

    // All preference operations and their callbacks run on the main queue.
    private var operation: VpnOperation?
    private var generation: UInt64 = 0
    private var uncertainSession: String?
    private var uncertainReason: String?
    private let preferenceMutation = VpnPreferenceMutation()

    private func directory() throws -> URL {
        guard let group = Bundle.main.object(forInfoDictionaryKey: "PolarisAppGroup") as? String,
              let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
        else { throw vpnError("App Group is unavailable. Sign the iOS App and Packet Tunnel with the same group.") }
        let directory = container.appendingPathComponent("polaris", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }
    @objc func sharedDirectory(_ invoke: Invoke) {
        do { invoke.resolve(["path": try directory().path]) } catch { invoke.reject(error.localizedDescription) }
    }

    /// Read-only status must not manufacture a manager or save preferences.
    private func loadManager(create: Bool, _ completion: @escaping (Result<LoadedVpnManager, Error>) -> Void) {
        guard let bundleID = Bundle.main.bundleIdentifier else { completion(.failure(vpnError("App bundle ID is unavailable"))); return }
        let providerID = bundleID + ".PacketTunnel"
        NETunnelProviderManager.loadAllFromPreferences { managers, error in
            DispatchQueue.main.async {
                if let error { completion(.failure(error)); return }
                let matches = (managers ?? []).filter {
                    ($0.protocolConfiguration as? NETunnelProviderProtocol)?.providerBundleIdentifier == providerID
                }
                guard matches.count <= 1 else {
                    completion(.failure(vpnError("Multiple Polaris VPN profiles exist; remove the duplicate in Settings."))); return
                }
                if let manager = matches.first { completion(.success(LoadedVpnManager(manager: manager, existing: true))); return }
                guard create else { completion(.success(LoadedVpnManager(manager: nil, existing: false))); return }
                let manager = NETunnelProviderManager()
                let configuration = NETunnelProviderProtocol()
                configuration.providerBundleIdentifier = providerID
                configuration.serverAddress = "Polaris"
                manager.protocolConfiguration = configuration
                manager.localizedDescription = "Polaris"
                completion(.success(LoadedVpnManager(manager: manager, existing: false)))
            }
        }
    }

    private func begin(_ invoke: Invoke, kind: String, requestID: String) -> VpnOperation? {
        guard operation == nil else { invoke.reject("A VPN operation is already in progress"); return nil }
        guard kind != "start" || !preferenceMutation.pending else {
            invoke.reject("A previous VPN preference write has not completed; no new profile mutation can be accepted"); return nil
        }
        generation += 1
        let op = VpnOperation(invoke, generation: generation, kind: kind, requestID: freshVpnRequestID(requestID))
        operation = op
        // Deadline includes load/save/reload preferences, not only NE status changes.
        let deadline = DispatchWorkItem {
            guard self.current(op) else { return }
            if op.started || op.kind == "stop" || self.preferenceMutation.pending {
                self.uncertainSession = (op.manager?.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration?["sessionID"] as? String
                self.uncertainReason = "A VPN operation timed out; extension completion is unknown. Stop the VPN and inspect its lifecycle report before retrying."
            }
            if op.kind == "start", op.started { op.manager?.connection.stopVPNTunnel() }
            self.finish(op, error: vpnError("VPN \(kind) timed out; completion and cleanup are unknown. Check system VPN status before retrying."))
        }
        op.deadline = deadline
        DispatchQueue.main.asyncAfter(deadline: .now() + (kind == "start" ? 40 : 25), execute: deadline)
        return op
    }
    private func current(_ op: VpnOperation) -> Bool { operation === op && generation == op.generation }

    @objc func start(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(StartArgs.self)
            guard !args.configContent.isEmpty, !args.requestID.isEmpty else { throw vpnError("VPN configuration/request identity is empty") }
            let configURL = try directory().appendingPathComponent("sing-box-config.json")
            DispatchQueue.main.async {
                guard let op = self.begin(invoke, kind: "start", requestID: args.requestID) else { return }
                self.loadManager(create: true) { result in
                    guard self.current(op) else { return }
                    do {
                        let loaded = try result.get()
                        guard let manager = loaded.manager else { throw vpnError("VPN manager is unavailable") }
                        op.manager = manager
                        let before = try self.snapshot(manager, profileExists: loaded.existing)
                        guard before["active"] as? Bool != true else {
                            throw vpnError("A Polaris VPN session is already active. Stop it explicitly before starting another session.")
                        }
                        if let error = before["cleanupError"] as? String ?? before["lastError"] as? String { throw vpnError(error) }
                        let sessionID = UUID().uuidString
                        let digest = SHA256.hash(data: Data(args.configContent.utf8)).map { String(format: "%02x", $0) }.joined()
                        guard let configuration = manager.protocolConfiguration as? NETunnelProviderProtocol else { throw vpnError("Unexpected VPN protocol") }
                        configuration.providerConfiguration = ["sessionID": sessionID, "requestID": op.requestID, "configDigest": digest]
                        // Config bytes are written only after the previous manager is inactive.
                        try Data(args.configContent.utf8).write(to: configURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
                        manager.isEnabled = true
                        // Persist before handing the write to NE; its OS side
                        // effect can outlive the callback and this host process.
                        try self.writePreferenceIntent(op.requestID, pending: true)
                        guard self.preferenceMutation.begin(op.requestID) else { throw vpnError("Another VPN preference write is pending") }
                        manager.saveToPreferences { error in
                            DispatchQueue.main.async {
                                if self.preferenceMutation.finish(op.requestID) {
                                    do { try self.finishPreferenceIntent(op.requestID) }
                                    catch {
                                        self.uncertainReason = "The VPN preference write returned, but its pending intent could not be reconciled: " + error.localizedDescription
                                        if self.current(op) { self.finish(op, error: error) }
                                        return // uncertainty must not activate a real VPN
                                    }
                                }
                                guard self.current(op) else { return }
                                if let error { self.finish(op, error: error); return }
                                manager.loadFromPreferences { error in
                                    DispatchQueue.main.async {
                                        guard self.current(op) else { return }
                                        if let error { self.finish(op, error: error); return }
                                        do {
                                            self.observe(op, wanted: .connected)
                                            // Register observer before issuing the transition.
                                            try manager.connection.startVPNTunnel(options: [
                                                "sessionID": sessionID as NSString, "requestID": op.requestID as NSString,
                                                "configContent": args.configContent as NSString
                                            ])
                                            op.started = true
                                            self.check(op, wanted: .connected, allowInitialDisconnected: true)
                                        } catch { self.finish(op, error: error) }
                                    }
                                }
                            }
                        }
                    } catch { self.finish(op, error: error) }
                }
            }
        } catch { invoke.reject(error.localizedDescription) }
    }

    @objc func stop(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(StopArgs.self)
            DispatchQueue.main.async {
                guard let op = self.begin(invoke, kind: "stop", requestID: args.requestID) else { return }
                self.loadManager(create: false) { result in
                    guard self.current(op) else { return }
                    do {
                        guard let manager = try result.get().manager else { self.finish(op, snapshot: try self.snapshot(nil)); return }
                        op.manager = manager
                        try self.writeStopIntent(op)
                        let status = manager.connection.status
                        if status == .invalid || status == .disconnected { self.finishStopped(op); return }
                        self.prepareStop(op)
                    } catch {
                        self.uncertainSession = (op.manager?.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration?["sessionID"] as? String
                        self.uncertainReason = error.localizedDescription
                        self.finish(op, error: error)
                    }
                }
            }
        } catch { invoke.reject(error.localizedDescription) }
    }

    @objc func status(_ invoke: Invoke) {
        DispatchQueue.main.async {
            var completed = false
            let deadline = DispatchWorkItem {
                guard !completed else { return }; completed = true
                invoke.reject("Reading system VPN preferences timed out; state is unknown")
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 10, execute: deadline)
            self.loadManager(create: false) { result in
                guard !completed else { return }; completed = true; deadline.cancel()
                do { invoke.resolve(try self.snapshot(result.get().manager)) } catch { invoke.reject(error.localizedDescription) }
            }
        }
    }

    private func observe(_ op: VpnOperation, wanted: NEVPNStatus) {
        guard let manager = op.manager else { return }
        op.observer = NotificationCenter.default.addObserver(forName: .NEVPNStatusDidChange, object: manager.connection, queue: .main) { _ in
            self.check(op, wanted: wanted)
        }
    }
    private func prepareStop(_ op: VpnOperation) {
        guard current(op), let manager = op.manager else { return }
        var requested = false
        let stop = {
            guard self.current(op), !requested else { return }
            requested = true
            self.observe(op, wanted: .disconnected)
            manager.connection.stopVPNTunnel()
            self.check(op, wanted: .disconnected)
        }
        // An unresponsive Go worker must not prevent the user asking NE to stop.
        DispatchQueue.main.asyncAfter(deadline: .now() + 2, execute: stop)
        guard let values = (manager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration,
              let session = manager.connection as? NETunnelProviderSession else { stop(); return }
        var context = values
        context["command"] = "prepareStop"
        context["stopRequestID"] = op.requestID
        do {
            try session.sendProviderMessage(JSONSerialization.data(withJSONObject: context)) { _ in
                DispatchQueue.main.async { stop() }
            }
        } catch { stop() }
    }
    private func check(_ op: VpnOperation, wanted: NEVPNStatus, allowInitialDisconnected: Bool = false) {
        guard current(op), let manager = op.manager else { return }
        let status = manager.connection.status
        if wanted == .disconnected, status == .disconnected || status == .invalid { finishStopped(op); return }
        if wanted == .connected, status == .connected {
            do {
                let observed = try snapshot(manager)
                guard observed["ownership"] as? String == "ownedSessionReported",
                      observed["lifecycle"] as? String == "running",
                      observed["runtimeStopped"] as? Bool == false,
                      observed["cleanupError"] == nil, observed["lastError"] == nil else {
                    uncertainSession = observed["sessionId"] as? String
                    let reason = "System VPN connected without a matching successful extension session report; inspect Settings before retrying."
                    uncertainReason = reason
                    throw vpnError(reason)
                }
                finish(op, snapshot: observed)
            } catch { finish(op, error: error) }
        } else if wanted == .connected, !allowInitialDisconnected, status == .disconnected || status == .invalid {
            manager.connection.fetchLastDisconnectError { error in
                DispatchQueue.main.async {
                    guard self.current(op) else { return }
                    let report = try? self.snapshot(manager)
                    let message = report?["cleanupError"] as? String ?? report?["lastError"] as? String
                    self.finish(op, error: error ?? vpnError(message ?? "The packet tunnel stopped before connecting."))
                }
            }
        }
    }
    private func finishStopped(_ op: VpnOperation) {
        do {
            var status = try snapshot(op.manager, stopRequestID: op.requestID, includeHostUncertainty: false)
            if status["runtimeStopped"] as? Bool == true, status["cleanupError"] == nil, status["lastError"] == nil {
                uncertainSession = nil; uncertainReason = nil
            } else if let uncertainReason { status["lastError"] = uncertainReason }
            if let failure = vpnStopFailure(evidence: status, profileExists: op.manager != nil) { throw vpnError(failure) }
            finish(op, snapshot: status)
        } catch {
            uncertainSession = (op.manager?.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration?["sessionID"] as? String
            uncertainReason = error.localizedDescription
            finish(op, error: error)
        }
    }

    private func snapshot(_ manager: NETunnelProviderManager?, stopRequestID: String? = nil, includeHostUncertainty: Bool = true, profileExists: Bool? = nil) throws -> [String: Any] {
        let ne = manager?.connection.status ?? .invalid
        let active = ne == .connecting || ne == .connected || ne == .reasserting || ne == .disconnecting
        let exists = profileExists ?? (manager != nil)
        var result: [String: Any] = ["status": ne.rawValue, "running": ne == .connected || ne == .reasserting,
                                    "active": active, "profileExists": exists, "ownership": exists ? "profileMatchedUnattested" : "unobserved",
                                    "cleanupEvidence": "CleanupUnknown"]
        let preferenceError = try pendingPreferenceIntent()
        guard let values = (manager?.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration else {
            if includeHostUncertainty, uncertainSession == nil { result["lastError"] = uncertainReason }
            if let preferenceError { result["lastError"] = preferenceError }
            if result["lastError"] == nil, let error = vpnIdleFailure(evidence: result, profileExists: exists, active: active) { result["lastError"] = error }
            return result
        }
        let url = try directory().appendingPathComponent("tunnel-lifecycle.json")
        let report: [String: Any]?
        if FileManager.default.fileExists(atPath: url.path) {
            report = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
        } else { report = nil }
        let expectedStop = try stopRequestID ?? stopIntent(for: values)
        result.merge(vpnSessionEvidence(profileIdentity: values, report: report, stopRequestID: expectedStop)) { _, evidence in evidence }
        if includeHostUncertainty, uncertainReason != nil,
           uncertainSession == nil || values["sessionID"] as? String == uncertainSession { result["lastError"] = uncertainReason }
        if let preferenceError { result["lastError"] = preferenceError }
        if result["lastError"] == nil, let error = vpnIdleFailure(evidence: result, profileExists: exists, active: active) { result["lastError"] = error }
        return result
    }

    private func writePreferenceIntent(_ requestID: String, pending: Bool) throws {
        let url = try directory().appendingPathComponent("vpn-preference-intent.json")
        try JSONSerialization.data(withJSONObject: ["requestID": requestID, "pending": pending]).write(to: url,
            options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
    private func finishPreferenceIntent(_ requestID: String) throws {
        let url = try directory().appendingPathComponent("vpn-preference-intent.json")
        guard let object = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any],
              object["requestID"] as? String == requestID else { return }
        try writePreferenceIntent(requestID, pending: false)
    }
    private func pendingPreferenceIntent() throws -> String? {
        let url = try directory().appendingPathComponent("vpn-preference-intent.json")
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        guard let object = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any],
              let pending = object["pending"] as? Bool else { throw vpnError("VPN preference intent is unreadable; state is unknown") }
        return pending ? "A VPN preference write was submitted without a reconciled completion; no new profile may be created or changed." : nil
    }
    private func writeStopIntent(_ op: VpnOperation) throws {
        var values = (op.manager?.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration ?? [:]
        values["stopRequestID"] = op.requestID
        let url = try directory().appendingPathComponent("vpn-stop-intent.json")
        try JSONSerialization.data(withJSONObject: values).write(to: url,
            options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
    private func stopIntent(for values: [String: Any]) throws -> String? {
        let url = try directory().appendingPathComponent("vpn-stop-intent.json")
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        guard let object = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any] else { throw vpnError("VPN stop intent is unreadable") }
        guard object["sessionID"] as? String == values["sessionID"] as? String,
              object["requestID"] as? String == values["requestID"] as? String,
              object["configDigest"] as? String == values["configDigest"] as? String else { return nil }
        guard let requestID = object["stopRequestID"] as? String, !requestID.isEmpty else { throw vpnError("VPN stop intent has no request identity") }
        return requestID
    }

    private func finish(_ op: VpnOperation, error: Error? = nil, snapshot: [String: Any]? = nil) {
        guard current(op) else { return }
        operation = nil
        op.deadline?.cancel()
        if let observer = op.observer { NotificationCenter.default.removeObserver(observer) }
        if let error { op.invoke.reject(error.localizedDescription) }
        else { op.invoke.resolve(snapshot ?? ["cleanupEvidence": "CleanupUnknown"]) }
    }
}
private func vpnError(_ message: String) -> NSError {
    NSError(domain: "PolarisVPN", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
}
@_cdecl("init_plugin_polaris_ios")
func initPlugin() -> Plugin { VpnPlugin() }
