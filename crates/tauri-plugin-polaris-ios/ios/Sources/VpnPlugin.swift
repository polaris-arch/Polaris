import CryptoKit
import Foundation
import NetworkExtension
import Tauri
import WebKit
import UIKit

private struct StartArgs: Decodable { let configContent: String; let requestID: String }
private struct StopArgs: Decodable {
    let requestID: String; let observationNonce: String
    let expectedStartRequestID: String?; let expectedSessionID: String?
    let expectedConfigDigest: String?; let expectedExtensionGeneration: UInt64?
}
private struct CurrentObserveArgs: Decodable { let observationNonce: String }
private struct RevokeArgs: Decodable { let expectedStartRequestID: String; let stopRequestID: String }
private struct ObserveArgs: Decodable {
    let sessionID: String; let requestID: String; let configDigest: String
    let extensionGeneration: UInt64; let observationNonce: String
    var identity: [String: Any] { ["sessionID": sessionID, "requestID": requestID, "configDigest": configDigest] }
}
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
    var completed = false
    var readingReady = false
    var identity: [String: Any]?
    var startIntent: VpnStartIntent?
    var observer: NSObjectProtocol?
    var deadline: DispatchWorkItem?
    var stopArgs: StopArgs?
    var awaitingStopObservation = false
    var stopObservation: [String: Any]?
    var readingStopped = false
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
    private let startAdmission = VpnStartAdmission()
    private var submittedStart: VpnStartIntent?

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
        let op = VpnOperation(invoke, generation: generation, kind: kind, requestID: requestID)
        if kind == "start" {
            guard let intent = startAdmission.admit(requestID) else {
                invoke.reject("StartCancelled: This VPN start request was revoked before dispatch"); return nil
            }
            op.startIntent = intent
        }
        operation = op
        // Deadline includes load/save/reload preferences, not only NE status changes.
        let deadline = DispatchWorkItem {
            guard self.current(op) else { return }
            if op.started || op.kind == "stop" || self.preferenceMutation.pending {
                self.uncertainSession = (op.manager?.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration?["sessionID"] as? String
                self.uncertainReason = "A VPN operation timed out; extension completion is unknown. Stop the VPN and inspect its lifecycle report before retrying."
            }
            if op.kind == "start", let intent = op.startIntent, op.started {
                self.stopSubmittedStart(intent, stopRequestID: freshVpnRequestID("timeout"))
            }
            self.finish(op, error: vpnError("StartupFailed: VPN \(kind) timed out; completion and cleanup are unknown. Check system VPN status before retrying."), noStoreEligible: false)
        }
        op.deadline = deadline
        DispatchQueue.main.asyncAfter(deadline: .now() + (kind == "start" ? 40 : 25), execute: deadline)
        return op
    }
    private func current(_ op: VpnOperation) -> Bool {
        operation === op && generation == op.generation && !op.completed
            && (op.startIntent?.allowsContinuation ?? true)
    }

    private func requireForeground() throws {
        if let failure = vpnForegroundFailure(appActive: UIApplication.shared.applicationState == .active,
            foregroundScene: UIApplication.shared.connectedScenes.contains(where: { $0.activationState == .foregroundActive })) {
            throw vpnError(failure)
        }
    }

    @objc func start(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(StartArgs.self)
            guard !args.configContent.isEmpty, !args.requestID.isEmpty else { throw vpnError("VPN configuration/request identity is empty") }
            let configURL = try directory().appendingPathComponent("sing-box-config.json")
            DispatchQueue.main.async {
                do { try self.requireForeground() } catch { invoke.reject(error.localizedDescription); return }
                guard let op = self.begin(invoke, kind: "start", requestID: args.requestID) else { return }
                self.loadManager(create: true) { result in
                    guard self.current(op) else { return }
                    do {
                        let loaded: LoadedVpnManager
                        do { loaded = try result.get() } catch { throw vpnError(vpnStartFailure(error, stage: "load")) }
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
                        op.identity = configuration.providerConfiguration
                        try self.requireForeground()
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
                                if let error { self.finish(op, error: vpnError(vpnStartFailure(error, stage: "save"))); return }
                                manager.loadFromPreferences { error in
                                    DispatchQueue.main.async {
                                        guard self.current(op) else { return }
                                        if let error { self.finish(op, error: vpnError(vpnStartFailure(error, stage: "reload"))); return }
                                        do {
                                            try self.requireForeground()
                                            guard let expected = op.identity,
                                                  vpnIdentityMatches(expected, (manager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration)
                                            else { throw vpnError("ReadyUnknown: Saved VPN profile changed before start submission") }
                                            self.observe(op, wanted: .connected)
                                            // Register observer before issuing the transition.
                                            // A throwing submission can still have an OS effect.
                                            // Mark the original intent before crossing that boundary.
                                            op.started = true
                                            op.startIntent?.submitted(expected)
                                            self.submittedStart = op.startIntent
                                            try manager.connection.startVPNTunnel(options: [
                                                "sessionID": sessionID as NSString, "requestID": op.requestID as NSString,
                                                "configContent": args.configContent as NSString
                                            ])
                                            self.check(op, wanted: .connected, allowInitialDisconnected: true)
                                        } catch {
                                            let message = error.localizedDescription
                                            self.finish(op, error: message.hasPrefix("ForegroundRequired:") || message.hasPrefix("ReadyUnknown:")
                                                ? error : vpnError(vpnStartFailure(error, stage: "start")))
                                        }
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
            guard !args.requestID.isEmpty, !args.observationNonce.isEmpty else { throw vpnError("Stop observation identity is empty") }
            DispatchQueue.main.async {
                guard let op = self.begin(invoke, kind: "stop", requestID: args.requestID) else { return }
                op.stopArgs = args
                self.loadManager(create: false) { result in
                    guard self.current(op) else { return }
                    do {
                        guard let manager = try result.get().manager else {
                            guard args.expectedStartRequestID == nil else { throw vpnError("CleanupUnknown: The original start manager is unavailable") }
                            self.finish(op, snapshot: try self.snapshot(nil)); return
                        }
                        op.manager = manager
                        let values = (manager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration
                        guard args.expectedStartRequestID == nil || values?["requestID"] as? String == args.expectedStartRequestID,
                              args.expectedSessionID == nil || values?["sessionID"] as? String == args.expectedSessionID,
                              args.expectedConfigDigest == nil || values?["configDigest"] as? String == args.expectedConfigDigest
                        else { throw vpnError("CleanupUnknown: Stop addresses a different original start session") }
                        op.identity = values
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

    @objc func revokePendingStart(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(RevokeArgs.self)
            guard !args.expectedStartRequestID.isEmpty, !args.stopRequestID.isEmpty else {
                throw vpnError("VPN revocation request identity is empty")
            }
            DispatchQueue.main.async {
                let pending = self.operation.flatMap { $0.kind == "start" && $0.requestID == args.expectedStartRequestID ? $0 : nil }
                let submitted = self.submittedStart.flatMap { $0.requestID == args.expectedStartRequestID ? $0 : nil }
                self.startAdmission.revoke(args.expectedStartRequestID, pending: pending?.startIntent ?? submitted)
                if let op = pending {
                    if op.started || self.preferenceMutation.requestID == op.requestID {
                        self.uncertainSession = op.identity?["sessionID"] as? String
                        self.uncertainReason = "The start request was revoked; submitted system work and cleanup remain unknown."
                    }
                    // Complete the old invoke exactly once. Save callbacks still
                    // reconcile their own durable preference intent before returning.
                    self.finish(op, error: vpnError("StartCancelled: The user stopped this VPN start request"))
                    self.generation += 1
                }
                // A ready callback can win after Rust captured this exact request
                // but before revoke reached the main queue. Retain its submitted
                // intent so that race still closes only the addressed session.
                if let submitted {
                    self.uncertainSession = submitted.submittedIdentity?["sessionID"] as? String
                    self.uncertainReason = "The submitted start was revoked; extension cleanup remains unknown."
                    self.stopSubmittedStart(submitted, stopRequestID: args.stopRequestID)
                }
                invoke.resolve(["revoked": true, "cleanupEvidence": "CleanupUnknown"])
            }
        } catch { invoke.reject(error.localizedDescription) }
    }

    /// Re-load the addressed profile at the point of stop. Neither a stale
    /// manager nor a late revocation/deadline may stop a successor session.
    private func stopSubmittedStart(_ intent: VpnStartIntent, stopRequestID: String) {
        self.loadManager(create: false) { result in
            do {
                guard let manager = try result.get().manager,
                      let values = (manager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration,
                      intent.mayStop(values) else { return }
                try self.writeStopIntent(values: values, requestID: stopRequestID)
                if let session = manager.connection as? NETunnelProviderSession {
                    var message = values
                    message["command"] = "prepareStop"; message["stopRequestID"] = stopRequestID
                    // This records the same stop intent in the extension when reachable.
                    try? session.sendProviderMessage(JSONSerialization.data(withJSONObject: message)) { _ in }
                }
                // No deferred callback may issue this transition against B.
                guard intent.mayStop((manager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration) else { return }
                manager.connection.stopVPNTunnel()
            } catch {
                self.uncertainSession = intent.submittedIdentity?["sessionID"] as? String
                self.uncertainReason = "The revoked start could not be reconciled: " + error.localizedDescription
            }
        }
    }

    @objc func observeSession(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(ObserveArgs.self)
            guard args.extensionGeneration > 0, !args.observationNonce.isEmpty,
                  vpnIdentityMatches(args.identity, args.identity) else { throw vpnError("ReadyUnknown: Session observation binding is empty") }
            DispatchQueue.main.async {
                var completed = false
                let deadline = DispatchWorkItem {
                    guard !completed else { return }; completed = true
                    invoke.reject("ReadyUnknown: Loading the bound VPN profile timed out")
                }
                DispatchQueue.main.asyncAfter(deadline: .now() + 10, execute: deadline)
                self.loadManager(create: false) { result in
                    guard !completed else { return }; completed = true; deadline.cancel()
                    do {
                        guard let manager = try result.get().manager else { throw vpnError("ReadyUnknown: VPN profile is unavailable") }
                        self.readLiveSession(manager, expectedIdentity: args.identity,
                            nonce: args.observationNonce, expectedGeneration: args.extensionGeneration) { result in
                            switch result {
                            case .success(let snapshot): invoke.resolve(["observationNonce": args.observationNonce, "snapshot": snapshot])
                            case .failure(let error): invoke.reject(error.localizedDescription)
                            }
                        }
                    } catch { invoke.reject(error.localizedDescription) }
                }
            }
        } catch { invoke.reject(error.localizedDescription) }
    }

    /// Cold hosts may observe an existing actual provider, never create a profile
    /// or recreate a local MainBirthToken from its saved status file.
    @objc func observeCurrentSession(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(CurrentObserveArgs.self)
            guard !args.observationNonce.isEmpty else { throw vpnError("ReadyUnknown: Observation nonce is empty") }
            DispatchQueue.main.async {
                self.loadManager(create: false) { result in
                    do {
                        guard self.operation == nil, let manager = try result.get().manager,
                              let identity = (manager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration,
                              vpnIdentityMatches(identity, identity) else { throw vpnError("ReadyUnknown: No current original manager") }
                        self.readLiveSession(manager, expectedIdentity: identity, nonce: args.observationNonce) { result in
                            switch result {
                            case .success(let snapshot): invoke.resolve(["observationNonce": args.observationNonce, "snapshot": snapshot])
                            case .failure(let error): invoke.reject(error.localizedDescription)
                            }
                        }
                    } catch { invoke.reject(error.localizedDescription) }
                }
            }
        } catch { invoke.reject(error.localizedDescription) }
    }

    /// A fresh provider response replaces file evidence for ordinary readiness.
    /// Re-check both the loaded profile and its NE status after the response.
    private func readLiveSession(_ manager: NETunnelProviderManager, expectedIdentity: [String: Any],
                                 nonce: String, expectedGeneration: UInt64? = nil,
                                 completion: @escaping (Result<[String: Any], Error>) -> Void) {
        let values = (manager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration
        guard manager.connection.status == .connected, vpnIdentityMatches(expectedIdentity, values),
              let session = manager.connection as? NETunnelProviderSession else {
            completion(.failure(vpnError("ReadyUnknown: The current system VPN does not match the bound session"))); return
        }
        var completed = false
        let finish: (Result<[String: Any], Error>) -> Void = { result in
            guard !completed else { return }; completed = true; completion(result)
        }
        let deadline = DispatchWorkItem { finish(.failure(vpnError("ReadyUnknown: Bound extension observation timed out"))) }
        DispatchQueue.main.asyncAfter(deadline: .now() + 8, execute: deadline)
        let message = vpnObservationMessage(identity: expectedIdentity, nonce: nonce, expectedGeneration: expectedGeneration)
        do {
            try session.sendProviderMessage(JSONSerialization.data(withJSONObject: message)) { data in
                DispatchQueue.main.async {
                    guard !completed else { return }
                    do {
                        guard let data,
                              let observation = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                              let evidence = vpnLiveEvidence(
                                profileIdentity: (manager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration,
                                expectedIdentity: expectedIdentity, observation: observation, nonce: nonce,
                                expectedGeneration: expectedGeneration) else {
                            throw vpnError("ReadyUnknown: Extension observation changed or reported an uncertain session")
                        }
                        // The saved profile can change while a provider response is
                        // in flight. Load its current manager again before granting.
                        self.loadManager(create: false) { result in
                            guard !completed else { return }
                            do {
                                guard let currentManager = try result.get().manager,
                                      currentManager.connection.status == .connected,
                                      vpnIdentityMatches(expectedIdentity,
                                        (currentManager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration),
                                      self.operation == nil || self.operation?.requestID == (expectedIdentity["requestID"] as? String),
                                      self.submittedStart == nil || (self.submittedStart?.requestID == (expectedIdentity["requestID"] as? String)
                                        && self.submittedStart?.revoked == false),
                                      self.uncertainReason == nil || (self.uncertainSession != nil && self.uncertainSession != (expectedIdentity["sessionID"] as? String)),
                                      try self.pendingPreferenceIntent() == nil else {
                                    throw vpnError("ReadyUnknown: The current VPN manager changed during observation")
                                }
                                var snapshot: [String: Any] = ["status": currentManager.connection.status.rawValue,
                                    "running": true, "active": true, "profileExists": true]
                                snapshot.merge(evidence) { _, fresh in fresh }
                                deadline.cancel(); finish(.success(snapshot))
                            } catch { deadline.cancel(); finish(.failure(vpnError("ReadyUnknown: " + error.localizedDescription))) }
                        }
                    } catch { deadline.cancel(); finish(.failure(vpnError("ReadyUnknown: " + error.localizedDescription))) }
                }
            }
        } catch { deadline.cancel(); finish(.failure(vpnError("ReadyUnknown: " + error.localizedDescription))) }
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
            // The original two-second fallback must still permit ordinary Stop
            // when a provider never answers. It supplies no scoped retirement.
            op.awaitingStopObservation = false
            self.loadManager(create: false) { result in
                guard self.current(op) else { return }
                do {
                    guard let currentManager = try result.get().manager, let expected = op.identity,
                          vpnIdentityMatches(expected, (currentManager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration)
                    else { throw vpnError("CleanupUnknown: The current manager changed before Stop") }
                    op.manager = currentManager
                    self.observe(op, wanted: .disconnected)
                    currentManager.connection.stopVPNTunnel()
                    self.check(op, wanted: .disconnected)
                } catch { self.finish(op, error: error) }
            }
        }
        // An unresponsive Go worker must not prevent the user asking NE to stop.
        DispatchQueue.main.asyncAfter(deadline: .now() + 2, execute: stop)
        guard let values = (manager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration,
              let session = manager.connection as? NETunnelProviderSession else { stop(); return }
        var context = values
        context["command"] = "prepareStop"
        context["stopRequestID"] = op.requestID
        context["observationNonce"] = op.stopArgs?.observationNonce
        if let expectedGeneration = op.stopArgs?.expectedExtensionGeneration { context["extensionGeneration"] = expectedGeneration }
        op.awaitingStopObservation = true
        do {
            try session.sendProviderMessage(JSONSerialization.data(withJSONObject: context)) { data in
                DispatchQueue.main.async {
                    // The callback belongs only to this original operation, even
                    // after its caller timed out. It cannot finish a successor.
                    op.stopObservation = data.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
                    op.awaitingStopObservation = false
                    guard self.current(op) else { return }
                    stop()
                    self.check(op, wanted: .disconnected)
                }
            }
        } catch { op.awaitingStopObservation = false; stop() }
    }
    private func check(_ op: VpnOperation, wanted: NEVPNStatus, allowInitialDisconnected: Bool = false) {
        guard current(op), let manager = op.manager else { return }
        let status = manager.connection.status
        if wanted == .disconnected, status == .disconnected || status == .invalid { finishStopped(op); return }
        if wanted == .connected, status == .connected {
            guard !op.readingReady, let identity = op.identity else { return }
            op.readingReady = true
            readLiveSession(manager, expectedIdentity: identity, nonce: freshVpnRequestID("start-observe")) { result in
                guard self.current(op) else { return }
                switch result {
                case .success(let observed): self.finish(op, snapshot: observed)
                case .failure(let error):
                    self.uncertainSession = identity["sessionID"] as? String
                    self.uncertainReason = error.localizedDescription
                    self.finish(op, error: error)
                }
            }
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
        guard current(op), !op.awaitingStopObservation, !op.readingStopped else { return }
        if let observation = op.stopObservation, let expected = op.identity, let args = op.stopArgs {
            op.readingStopped = true
            loadManager(create: false) { result in
                guard self.current(op) else { return }
                do {
                    guard let manager = try result.get().manager,
                          manager.connection.status == .disconnected || manager.connection.status == .invalid,
                          vpnIdentityMatches(expected, (manager.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration),
                          observation["observationNonce"] as? String == args.observationNonce,
                          let report = observation["report"] as? [String: Any],
                          vpnIdentityMatches(expected, report["identity"] as? [String: Any])
                    else { throw vpnError("CleanupUnknown: Live Stop observation changed original manager") }
                    var snapshot: [String: Any] = ["status": manager.connection.status.rawValue,
                        "running": false, "active": false, "profileExists": true]
                    let evidence = vpnRetirementEvidence(profileIdentity: expected, expectedIdentity: expected,
                        observation: observation, nonce: args.observationNonce, stopRequestID: args.requestID,
                        expectedGeneration: args.expectedExtensionGeneration)
                        ?? vpnSessionEvidence(profileIdentity: expected, report: report, stopRequestID: args.requestID)
                    snapshot.merge(evidence) { _, live in live }
                    if let failure = vpnStopFailure(evidence: snapshot, profileExists: true) { throw vpnError(failure) }
                    self.uncertainSession = nil; self.uncertainReason = nil
                    self.finish(op, snapshot: snapshot)
                } catch { self.finish(op, error: error) }
            }
            return
        }
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
        try writeStopIntent(values: (op.manager?.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration ?? [:], requestID: op.requestID)
    }
    private func writeStopIntent(values: [String: Any], requestID: String) throws {
        var values = values
        values["stopRequestID"] = requestID
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

    private func finish(_ op: VpnOperation, error: Error? = nil, snapshot: [String: Any]? = nil, noStoreEligible: Bool = true) {
        guard operation === op, generation == op.generation, !op.completed else { return }
        op.completed = true
        op.startIntent?.finish()
        operation = nil
        op.deadline?.cancel()
        if let observer = op.observer { NotificationCenter.default.removeObserver(observer) }
        if let error, noStoreEligible, let terminal = op.startIntent?.noStoreTerminal() {
            op.invoke.resolve(["nativeNoStoreTerminal": terminal, "startError": error.localizedDescription])
        }
        else if let error { op.invoke.reject(error.localizedDescription) }
        else { op.invoke.resolve(snapshot ?? ["cleanupEvidence": "CleanupUnknown"]) }
    }
}
private func vpnError(_ message: String) -> NSError {
    NSError(domain: "PolarisVPN", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
}
@_cdecl("init_plugin_polaris_ios")
func initPlugin() -> Plugin { VpnPlugin() }
