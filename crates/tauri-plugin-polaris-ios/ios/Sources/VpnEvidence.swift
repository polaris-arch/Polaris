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
    init(_ requestID: String) { self.requestID = requestID }
    var allowsContinuation: Bool { !revoked && !terminal }
    func submitted(_ identity: [String: Any]) { submittedIdentity = identity }
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
    return vpnSessionEvidence(profileIdentity: profileIdentity, report: report)
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
