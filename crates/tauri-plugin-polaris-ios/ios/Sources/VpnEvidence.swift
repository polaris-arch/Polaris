import Foundation

// Rust generations restart with the host process. A native nonce identifies the
// actual NE request independently of that process-local ordering counter.
func freshVpnRequestID(_ callerID: String) -> String { callerID + ":" + UUID().uuidString }

final class VpnPreferenceMutation {
    private(set) var requestID: String?
    var pending: Bool { requestID != nil }
    func begin(_ requestID: String) -> Bool {
        guard self.requestID == nil else { return false }
        self.requestID = requestID
        return true
    }
    @discardableResult
    func finish(_ requestID: String) -> Bool {
        guard self.requestID == requestID else { return false }
        self.requestID = nil
        return true
    }
}

/// Main-queue admission seam used by the real load/save/reload/start callbacks.
/// Revocation may arrive before Tauri dispatches the corresponding Start.
final class VpnStartAdmission {
    private var revokedRequests = Set<String>()
    private var saturated = false
    func admit(_ requestID: String) -> VpnStartIntent? {
        guard !saturated, !requestID.isEmpty, !revokedRequests.contains(requestID) else { return nil }
        return VpnStartIntent(requestID)
    }
    func revoke(_ requestID: String, pending: VpnStartIntent?) {
        if let pending, pending.requestID == requestID { pending.revoke() }
        if revokedRequests.contains(requestID) { return }
        else if revokedRequests.count < 64 { revokedRequests.insert(requestID) }
        else { saturated = true } // Never evict an old revoked request and admit its late dispatch.
    }
}

func vpnObservationMessage(identity: [String: Any], nonce: String, expectedGeneration: UInt64? = nil) -> [String: Any] {
    var message = identity
    message["command"] = "observeSession"; message["observationNonce"] = nonce
    if let expectedGeneration { message["extensionGeneration"] = expectedGeneration }
    return message
}

func vpnForegroundFailure(appActive: Bool, foregroundScene: Bool) -> String? {
    appActive && foregroundScene ? nil : "ForegroundRequired: Return to the foreground and explicitly retry the connection."
}

final class VpnStartIntent {
    let requestID: String
    private(set) var revoked = false
    private var terminal = false
    private(set) var submittedIdentity: [String: Any]?
    private(set) var submissionAttempted = false
    init(_ requestID: String) { self.requestID = requestID }
    var allowsContinuation: Bool { !revoked && !terminal }
    func submitted(_ identity: [String: Any]) { submissionAttempted = true; submittedIdentity = identity }
    func revoke() { revoked = true }
    func mayStop(_ profile: [String: Any]?) -> Bool {
        guard let submittedIdentity else { return false }
        return vpnIdentityMatches(submittedIdentity, profile)
    }
    @discardableResult func finish() -> Bool {
        guard !terminal else { return false }
        terminal = true
        return true
    }
    func noStoreTerminal() -> [String: Any]? {
        guard terminal, !submissionAttempted, submittedIdentity == nil else { return nil }
        return ["contractVersion": "polaris-ios-native-no-store-v1", "requestID": requestID,
                "startIntentFinished": true, "submissionAttempted": false, "storeConstruction": "NoStoreConstruction"]
    }
}

func vpnIdentityMatches(_ expected: [String: Any], _ actual: [String: Any]?) -> Bool {
    guard let actual else { return false }
    return ["sessionID", "requestID", "configDigest"].allSatisfy { key in
        guard let value = expected[key] as? String, !value.isEmpty else { return false }
        return actual[key] as? String == value
    }
}

/// A live message is ordinary readiness only; cleanup evidence stays unknown.
func vpnLiveEvidence(profileIdentity: [String: Any]?, expectedIdentity: [String: Any],
                     observation: [String: Any], nonce: String,
                     expectedGeneration: UInt64? = nil) -> [String: Any]? {
    guard !nonce.isEmpty, vpnIdentityMatches(expectedIdentity, profileIdentity),
          observation["observationNonce"] as? String == nonce,
          let report = observation["report"] as? [String: Any],
          vpnIdentityMatches(expectedIdentity, report["identity"] as? [String: Any]),
          let generation = report["generation"] as? NSNumber, generation.uint64Value > 0,
          expectedGeneration == nil || expectedGeneration == generation.uint64Value,
          report["lifecycle"] as? String == "running",
          report["runtimeStopped"] as? Bool == false,
          report["cleanupError"] == nil, report["lastError"] == nil,
          report["uncertainSettingsGeneration"] == nil else { return nil }
    var result = vpnSessionEvidence(profileIdentity: profileIdentity, report: report)
    if let scope = vpnTailscaleStoreExport(report: report, configDigest: expectedIdentity["configDigest"] as? String, terminal: false) {
        result["tailscaleStoreScope"] = scope
    }
    return result
}

private func vpnHex64(_ value: String) -> Bool {
    value.utf8.count == 64 && value.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
}

/// Validate the complete original Go export, never a caller-made single-node proof.
func vpnTailscaleStoreExport(report: [String: Any], configDigest: String?, terminal: Bool) -> [String: Any]? {
    guard let configDigest, vpnHex64(configDigest), let raw = report["tailscaleStoreScope"] as? String,
          raw.utf8.count <= 256 * 1024,
          let value = try? JSONSerialization.jsonObject(with: Data(raw.utf8)) as? [String: Any],
          Set(value.keys) == Set(["contractVersion", "globalCleanupEvidence", "instances"]),
          value["contractVersion"] as? String == "polaris-ts-auth-writer-retirement-v1",
          value["globalCleanupEvidence"] as? String == "CleanupUnknown",
          let instances = value["instances"] as? [[String: Any]], !instances.isEmpty, instances.count <= 256,
          instances.contains(where: { $0["configDigest"] as? String == configDigest }) else { return nil }
    var nonces = Set<String>()
    for instance in instances {
        guard Set(instance.keys) == Set(["runNonce", "configDigest", "terminal", "censusComplete", "nodes"]),
              let nonce = instance["runNonce"] as? String, vpnHex64(nonce), nonces.insert(nonce).inserted,
              let runDigest = instance["configDigest"] as? String, vpnHex64(runDigest),
              let state = instance["terminal"] as? String, ["Unknown", "SealedDrained", "NoStoreConstruction"].contains(state),
              let complete = instance["censusComplete"] as? Bool,
              let nodes = instance["nodes"] as? [[String: Any]],
              !terminal || (complete && state != "Unknown") else { return nil }
        var tags = Set<String>(), directories = Set<String>()
        var sealed = false
        for node in nodes {
            guard Set(node.keys) == Set(["tag", "stateDirectory", "stateFile", "writerState", "stateFileState", "stateFileRevision", "profileState", "profileFingerprint"]),
                  let tag = node["tag"] as? String, !tag.isEmpty, tags.insert(tag).inserted,
                  let directory = node["stateDirectory"] as? String, directory.hasPrefix("/"), directory != "/",
                  !directory.contains("\0"), !directory.hasSuffix("/"), !directory.contains("//"),
                  !directory.split(separator: "/").contains(where: { $0 == "." || $0 == ".." }),
                  directories.insert(directory).inserted,
                  node["stateFile"] as? String == directory + "/tailscaled.state",
                  let writer = node["writerState"] as? String, ["Unknown", "SealedDrained", "NoStoreConstruction"].contains(writer),
                  let file = node["stateFileState"] as? String, ["Unknown", "Missing", "Regular"].contains(file),
                  let revision = node["stateFileRevision"] as? String,
                  (file == "Regular" ? vpnHex64(revision) : revision.isEmpty),
                  let profile = node["profileState"] as? String, ["Unknown", "None", "Bound"].contains(profile),
                  let fingerprint = node["profileFingerprint"] as? String,
                  (profile == "Bound" ? (vpnHex64(fingerprint) && file == "Regular") : fingerprint.isEmpty),
                  !terminal || writer != "Unknown" else { return nil }
            sealed = sealed || writer == "SealedDrained"
        }
        if terminal && state != (sealed ? "SealedDrained" : "NoStoreConstruction") { return nil }
    }
    return value
}

/// Only a fresh deferred prepareStop response can carry this field. File snapshots
/// continue to use vpnSessionEvidence and cannot manufacture scoped retirement.
func vpnRetirementEvidence(profileIdentity: [String: Any]?, expectedIdentity: [String: Any],
                           observation: [String: Any], nonce: String, stopRequestID: String,
                           expectedGeneration: UInt64? = nil) -> [String: Any]? {
    guard !nonce.isEmpty, !stopRequestID.isEmpty, vpnIdentityMatches(expectedIdentity, profileIdentity),
          observation["observationNonce"] as? String == nonce,
          let report = observation["report"] as? [String: Any],
          vpnIdentityMatches(expectedIdentity, report["identity"] as? [String: Any]),
          report["operationRequestID"] as? String == stopRequestID,
          report["lifecycle"] as? String == "stopped", report["runtimeStopped"] as? Bool == true,
          report["cleanupError"] == nil, report["lastError"] == nil, report["uncertainSettingsGeneration"] == nil,
          let source = report["stopSourceGeneration"] as? NSNumber, source.uint64Value > 0, source.uint64Value < UInt64.max,
          let stopped = report["generation"] as? NSNumber, stopped.uint64Value == source.uint64Value + 1,
          expectedGeneration == nil || expectedGeneration == source.uint64Value,
          let export = vpnTailscaleStoreExport(report: report, configDigest: expectedIdentity["configDigest"] as? String, terminal: true)
    else { return nil }
    var result = vpnSessionEvidence(profileIdentity: profileIdentity, report: report, stopRequestID: stopRequestID)
    result["tailscaleStoreRetirement"] = ["contractVersion": "polaris-ios-ts-store-retirement-v1",
        "sessionID": expectedIdentity["sessionID"]!, "startRequestID": expectedIdentity["requestID"]!,
        "configDigest": expectedIdentity["configDigest"]!, "sourceExtensionGeneration": source,
        "stopExtensionGeneration": stopped, "stopRequestID": stopRequestID, "observationNonce": nonce,
        "globalCleanupEvidence": "CleanupUnknown", "storeRetirement": export]
    return result
}

/// Only explicit system error receipts classify a denial. A generic NE save
/// failure or any timeout remains StartupFailed/ReadyUnknown.
func vpnStartFailure(_ error: Error, stage: String) -> String {
    let system = error as NSError
    let underlying = system.userInfo[NSUnderlyingErrorKey] as? NSError
    let errors = [system] + (underlying.map { [$0] } ?? [])
    if stage == "save", errors.contains(where: {
        ($0.domain == NSCocoaErrorDomain && $0.code == NSUserCancelledError)
            || ($0.domain == NSPOSIXErrorDomain && ($0.code == 1 || $0.code == 13))
    }) { return "PermissionDenied: System VPN configuration permission was rejected. " + system.localizedDescription }
    return "StartupFailed: VPN \(stage) failed. " + system.localizedDescription
}

func vpnIdleFailure(evidence: [String: Any], profileExists: Bool, active: Bool) -> String? {
    guard !active else { return nil }
    return vpnStopFailure(evidence: evidence, profileExists: profileExists)
}

func vpnStopFailure(evidence: [String: Any], profileExists: Bool) -> String? {
    guard profileExists else { return nil }
    if let error = evidence["cleanupError"] as? String ?? evidence["lastError"] as? String {
        return "System VPN is disconnected; extension reported: " + error
    }
    guard evidence["ownership"] as? String == "ownedSessionReported",
          evidence["runtimeStopped"] as? Bool == true,
          evidence["lifecycle"] as? String == "stopped" else {
        return "System VPN is disconnected, but this stop request has no matching completed extension report; cleanup is unknown. Inspect Settings before retrying."
    }
    return nil
}

/// Report identity is independent from bundle/profile matching. The latter only
/// permits addressing that manager; it cannot prove Go resource ownership.
func vpnSessionEvidence(profileIdentity: [String: Any]?, report: [String: Any]?, stopRequestID: String? = nil) -> [String: Any] {
    var result: [String: Any] = ["ownership": "profileMatchedUnattested", "cleanupEvidence": "CleanupUnknown"]
    guard let expected = profileIdentity,
          let sessionID = expected["sessionID"] as? String,
          let requestID = expected["requestID"] as? String,
          let configDigest = expected["configDigest"] as? String else { return result }
    result["sessionId"] = sessionID; result["requestId"] = requestID; result["configDigest"] = configDigest
    guard let report, let identity = report["identity"] as? [String: Any],
          identity["sessionID"] as? String == sessionID,
          identity["requestID"] as? String == requestID,
          identity["configDigest"] as? String == configDigest else { return result }
    result["ownership"] = "ownedSessionReported"
    result["lifecycle"] = report["lifecycle"]
    result["cleanupError"] = report["cleanupError"]
    result["lastError"] = report["lastError"]
    result["extensionGeneration"] = report["generation"]
    result["uncertainSettingsGeneration"] = report["uncertainSettingsGeneration"]
    // A prior stopped file may describe the same session but not this stop request.
    if stopRequestID == nil || report["operationRequestID"] as? String == stopRequestID {
        result["runtimeStopped"] = report["runtimeStopped"]
    }
    return result
}

/// A repeat host Stop may observe the existing terminal report, but cannot
/// mint a new prepareStop/retirement receipt or change its durable request ID.
func vpnIdempotentStopIsConfirmed(evidence: [String: Any], expectedGeneration: UInt64?) -> Bool {
    guard evidence["active"] as? Bool == false,
          evidence["running"] as? Bool == false,
          let status = evidence["status"] as? Int, status == 0 || status == 1,
          evidence["ownership"] as? String == "ownedSessionReported",
          evidence["lifecycle"] as? String == "stopped",
          evidence["runtimeStopped"] as? Bool == true,
          evidence["cleanupError"] == nil, evidence["lastError"] == nil,
          evidence["uncertainSettingsGeneration"] == nil else { return false }
    if let expectedGeneration {
        guard expectedGeneration < UInt64.max,
              let stopped = evidence["extensionGeneration"] as? NSNumber,
              stopped.uint64Value == expectedGeneration + 1 else { return false }
    }
    return true
}

/// Called only with a raw session snapshot (host-only uncertainty excluded).
/// Provider/preference errors remain in that snapshot and cannot be cleared.
func vpnRecoverRepeatedStop(evidence: [String: Any], expectedGeneration: UInt64?,
                            uncertainSession: inout String?, uncertainReason: inout String?) -> Bool {
    guard vpnIdempotentStopIsConfirmed(evidence: evidence, expectedGeneration: expectedGeneration) else { return false }
    if let uncertainSession, uncertainSession != evidence["sessionId"] as? String { return false }
    uncertainSession = nil
    uncertainReason = nil
    return true
}

/// The actual host Stop catch records its own failure without rebinding an
/// already uncertain different session (or a broader unscoped uncertainty).
func vpnRecordStopFailure(sessionID: String?, reason: String,
                          uncertainSession: inout String?, uncertainReason: inout String?) {
    if uncertainSession != nil || uncertainReason != nil {
        guard uncertainSession == sessionID else { return }
    }
    uncertainSession = sessionID
    uncertainReason = reason
}

/// Independent directory ABI. Authentication exports are deliberately rejected.
func vpnTailscaleDirectoryExport(raw: String?, configDigest: String, terminal: Bool) -> [String: Any]? {
    guard vpnHex64(configDigest), let raw, raw.utf8.count <= 256 * 1024,
          let value = try? JSONSerialization.jsonObject(with: Data(raw.utf8)) as? [String: Any],
          Set(value.keys) == Set(["contractVersion", "globalCleanupEvidence", "instances"]),
          value["contractVersion"] as? String == "polaris-ts-directory-retirement-v1",
          value["globalCleanupEvidence"] as? String == "CleanupUnknown",
          let instances = value["instances"] as? [[String: Any]], !instances.isEmpty, instances.count <= 256,
          instances.contains(where: { $0["configDigest"] as? String == configDigest }) else { return nil }
    var nonces = Set<String>()
    for instance in instances {
        guard Set(instance.keys) == Set(["runNonce", "configDigest", "terminal", "censusComplete", "nodes"]),
              let nonce = instance["runNonce"] as? String, vpnHex64(nonce), nonces.insert(nonce).inserted,
              let runDigest = instance["configDigest"] as? String, vpnHex64(runDigest),
              instance["censusComplete"] as? Bool == true,
              let state = instance["terminal"] as? String,
              ["Unknown", "SealedDrained", "NoDirectoryConstruction"].contains(state),
              let nodes = instance["nodes"] as? [[String: Any]], nodes.count <= 256,
              !terminal || state == (nodes.isEmpty ? "NoDirectoryConstruction" : "SealedDrained") else { return nil }
        var tags = Set<String>(), directories = Set<String>()
        for node in nodes {
            guard Set(node.keys) == Set(["tag", "stateDirectory", "directoryIdentity", "incarnation", "directoryEpoch", "writerState", "censusVersion", "failure"]),
                  let tag = node["tag"] as? String, !tag.isEmpty, tags.insert(tag).inserted,
                  let directory = node["stateDirectory"] as? String, directory.hasPrefix("/"), directory != "/",
                  !directory.contains("\0"), !directory.hasSuffix("/"), !directory.contains("//"),
                  !directory.split(separator: "/").contains(where: { $0 == "." || $0 == ".." }),
                  directories.insert(directory).inserted,
                  let identity = node["directoryIdentity"] as? String,
                  let incarnation = node["incarnation"] as? String, incarnation.utf8.count == 32,
                  incarnation.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }),
                  let epoch = node["directoryEpoch"] as? String, epoch.utf8.count == 32,
                  epoch.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }),
                  node["censusVersion"] as? Int == 1,
                  let writer = node["writerState"] as? String, ["Unknown", "SealedDrained"].contains(writer),
                  let failure = node["failure"] as? String,
                  !terminal || (writer == "SealedDrained" && failure.isEmpty) else { return nil }
            let parts = identity.split(separator: ":", omittingEmptySubsequences: false)
            guard parts.count == 5, parts[0] == "unix",
                  parts.dropFirst().allSatisfy({ part in
                      guard let number = UInt64(part) else { return false }
                      return String(number) == part
                  }) else { return nil }
        }
    }
    return value
}

private func vpnDirectoryCensus(_ export: [String: Any]) -> Set<Data>? {
    guard let instances = export["instances"] as? [[String: Any]] else { return nil }
    var result = Set<Data>()
    for instance in instances {
        guard let nodes = instance["nodes"] as? [[String: Any]] else { return nil }
        let scopes = nodes.map { node in
            ["tag": node["tag"]!, "stateDirectory": node["stateDirectory"]!,
             "directoryIdentity": node["directoryIdentity"]!, "incarnation": node["incarnation"]!, "directoryEpoch": node["directoryEpoch"]!]
        }
        // Sort canonical node encodings so harmless export ordering is irrelevant.
        guard let encodedScopes = try? scopes.map({ try JSONSerialization.data(withJSONObject: $0, options: .sortedKeys) }) else { return nil }
        let canonical: [String: Any] = ["runNonce": instance["runNonce"]!, "configDigest": instance["configDigest"]!,
            "scopes": encodedScopes.map { $0.base64EncodedString() }.sorted()]
        guard let data = try? JSONSerialization.data(withJSONObject: canonical, options: .sortedKeys), result.insert(data).inserted else { return nil }
    }
    return result
}

/// Only a fresh original prepareStop response can mint this independent field.
/// Durable report files and ordinary stopped snapshots do not call this function.
func vpnDirectoryRetirementEvidence(profileIdentity: [String: Any]?, expectedIdentity: [String: Any],
                                    observation: [String: Any], nonce: String, stopRequestID: String,
                                    expectedGeneration: UInt64? = nil) -> [String: Any]? {
    guard !nonce.isEmpty, !stopRequestID.isEmpty, vpnIdentityMatches(expectedIdentity, profileIdentity),
          observation["observationNonce"] as? String == nonce,
          let report = observation["report"] as? [String: Any],
          vpnIdentityMatches(expectedIdentity, report["identity"] as? [String: Any]),
          report["operationRequestID"] as? String == stopRequestID,
          report["cleanupEvidence"] as? String == "CleanupUnknown",
          report["lifecycle"] as? String == "stopped", report["runtimeStopped"] as? Bool == true,
          report["cleanupError"] == nil, report["lastError"] == nil, report["uncertainSettingsGeneration"] == nil,
          let source = report["stopSourceGeneration"] as? NSNumber, source.uint64Value > 0, source.uint64Value < UInt64.max,
          let stopped = report["generation"] as? NSNumber, stopped.uint64Value == source.uint64Value + 1,
          expectedGeneration == nil || expectedGeneration == source.uint64Value,
          let digest = expectedIdentity["configDigest"] as? String,
          let original = vpnTailscaleDirectoryExport(raw: report["stopOriginalDirectoryScope"] as? String, configDigest: digest, terminal: false),
          let terminal = vpnTailscaleDirectoryExport(raw: report["tailscaleDirectoryScope"] as? String, configDigest: digest, terminal: true),
          let instances = terminal["instances"] as? [[String: Any]],
          instances.contains(where: { !($0["nodes"] as? [[String: Any]] ?? []).isEmpty }),
          let census = vpnDirectoryCensus(original), census == vpnDirectoryCensus(terminal) else { return nil }
    return ["tailscaleDirectoryRetirement": ["contractVersion": "polaris-ios-ts-directory-retirement-v1",
        "sessionID": expectedIdentity["sessionID"]!, "startRequestID": expectedIdentity["requestID"]!,
        "configDigest": digest, "sourceExtensionGeneration": source,
        "stopExtensionGeneration": stopped, "stopRequestID": stopRequestID, "observationNonce": nonce,
        "globalCleanupEvidence": "CleanupUnknown", "originalScope": original, "directoryRetirement": terminal]]
}

/// A source-pinned capability from the original no-run CommandServer. Neither
/// profile dictionaries nor generic readiness can supply this admission proof.
func vpnManagedBootstrapCapability(observation: [String: Any], expectedIdentity: [String: Any],
                                   nonce: String, expectedSource: String, root: String) -> [String: Any]? {
    guard !nonce.isEmpty, expectedSource.utf8.count == 40,
          expectedSource.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }),
          observation["contractVersion"] as? String == "polaris-ios-g4-bootstrap-v1",
          observation["observationNonce"] as? String == nonce,
          vpnIdentityMatches(expectedIdentity, observation["identity"] as? [String: Any]),
          let generation = observation["extensionGeneration"] as? NSNumber, generation.uint64Value > 0,
          let capability = observation["capability"] as? [String: Any],
          Set(capability.keys) == Set(["contractVersion", "sourceABI", "sourceIdentity", "managedRoot", "logicalRoot", "ownerNonce", "participating"]),
          capability["contractVersion"] as? String == "polaris-ts-directory-capability-v1",
          capability["sourceABI"] as? String == "polaris-g4-directory-v1",
          capability["sourceIdentity"] as? String == expectedSource,
          capability["managedRoot"] as? String == root + "/tailscale-g4-v1",
          capability["logicalRoot"] as? String == root + "/tailscale",
          capability["participating"] as? Bool == true,
          let owner = capability["ownerNonce"] as? String, owner.utf8.count == 32,
          owner.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else { return nil }
    return ["extensionGeneration": generation, "ownerNonce": owner]
}

func vpnStartOptions(sessionID: String, requestID: String, config: String, managed: Bool) -> [String: NSObject] {
    var options: [String: NSObject] = ["sessionID": sessionID as NSString, "requestID": requestID as NSString]
    if managed { options["directoryBootstrapVersion"] = "polaris-ios-g4-bootstrap-v1" as NSString }
    else { options["configContent"] = config as NSString }
    return options
}

// Ordinary Stop dispatch depends only on the original saved owner, not a live
// cleanup census or provider lifecycle phase. The caller must re-load manager.
func vpnMaySubmitOrdinaryStop(expected: [String: Any], actual: [String: Any]?) -> Bool {
    vpnIdentityMatches(expected, expected) && vpnIdentityMatches(expected, actual)
}

/// Serialized by the caller (the plugin uses the main queue). Stop dispatch
/// and the original worker's terminal reply have independent completion gates:
/// the two-second fallback must start Stop without consuming that reply.
func vpnPrepareOrdinaryStop<Manager>(
    expected: [String: Any], nonce: String, stopRequestID: String, expectedGeneration: UInt64?,
    isCurrent: @escaping () -> Bool,
    schedule: (TimeInterval, @escaping () -> Void) -> Void,
    observe: (@escaping (Data?) -> Void) throws -> Void,
    reload: @escaping (@escaping (Result<Manager, Error>) -> Void) -> Void,
    identity: @escaping (Manager) -> [String: Any]?,
    retainObservation: @escaping ([String: Any]?) -> Void,
    dispatch: @escaping (Manager) -> Void,
    failure: @escaping (Error) -> Void
) {
    var stopRequested = false
    var stopReloaded = false
    var terminalAccepted = false
    var checkingTerminal = false
    var unavailableReported = false
    var failed = false
    let fail: (Error) -> Void = { error in
        guard isCurrent(), !failed else { return }
        failed = true; failure(error)
    }
    let checkManager: (Manager) throws -> Void = { manager in
        guard vpnMaySubmitOrdinaryStop(expected: expected, actual: identity(manager)) else {
            throw NSError(domain: "Polaris.VpnStopDispatch", code: 1,
                userInfo: [NSLocalizedDescriptionKey: "CleanupUnknown: The current manager changed before Stop observation/dispatch"])
        }
    }
    let requestStop = {
        guard isCurrent(), !failed, !stopRequested else { return }
        stopRequested = true
        reload { result in
            guard isCurrent(), !failed, !stopReloaded else { return }
            stopReloaded = true
            do {
                let manager = try result.get()
                try checkManager(manager)
                dispatch(manager)
            } catch { fail(error) }
        }
    }
    let receive: (Data?) -> Void = { data in
        guard isCurrent(), !failed, !terminalAccepted, !checkingTerminal else { return }
        if !nonce.isEmpty, !stopRequestID.isEmpty, let data,
           let candidate = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           candidate["observationNonce"] as? String == nonce,
           let report = candidate["report"] as? [String: Any],
           vpnIdentityMatches(expected, report["identity"] as? [String: Any]),
           report["operationRequestID"] as? String == stopRequestID,
           ["stopped", "uncertain"].contains(report["lifecycle"] as? String ?? ""),
           report["runtimeStopped"] as? Bool == true,
           let source = report["stopSourceGeneration"] as? NSNumber,
           source.uint64Value > 0, source.uint64Value < UInt64.max,
           let stopped = report["generation"] as? NSNumber, stopped.uint64Value == source.uint64Value + 1,
           expectedGeneration == nil || expectedGeneration == source.uint64Value {
            checkingTerminal = true
            var observedManager = false
            // Re-load again for the reply: the manager may have changed since
            // the fallback dispatched Stop to the original connection.
            reload { result in
                guard isCurrent(), !failed, !observedManager else { return }
                observedManager = true
                do {
                    try checkManager(result.get())
                    terminalAccepted = true
                    // Retain original terminal errors/Unknown unchanged. Only
                    // the existing strict retirement validators can mint proof.
                    retainObservation(candidate)
                } catch { fail(error) }
            }
        } else if !unavailableReported {
            unavailableReported = true
            retainObservation(nil)
        }
        requestStop()
    }
    schedule(2, requestStop)
    do { try observe(receive) } catch { receive(nil) }
}
