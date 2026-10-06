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
          let instances = value["instances"] as? [[String: Any]], !instances.isEmpty else { return nil }
    var nonces = Set<String>()
    for instance in instances {
        guard Set(instance.keys) == Set(["runNonce", "configDigest", "terminal", "censusComplete", "nodes"]),
              let nonce = instance["runNonce"] as? String, vpnHex64(nonce), nonces.insert(nonce).inserted,
              instance["configDigest"] as? String == configDigest,
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
