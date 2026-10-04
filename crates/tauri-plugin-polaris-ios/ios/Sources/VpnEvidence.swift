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
    // A prior stopped file may describe the same session but not this stop request.
    if stopRequestID == nil || report["operationRequestID"] as? String == stopRequestID {
        result["runtimeStopped"] = report["runtimeStopped"]
    }
    return result
}
