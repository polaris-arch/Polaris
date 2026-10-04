import Foundation

struct TunnelIdentity: Codable, Equatable {
    let sessionID: String
    let requestID: String
    let configDigest: String
}

struct TunnelReport: Encodable {
    let identity: TunnelIdentity?
    let generation: UInt64
    let lifecycle: String
    let runtimeStopped: Bool?
    let operationRequestID: String?
    let uncertainSettingsGeneration: UInt64?
    // Neither a nil Go instance nor NE disconnected proves resource disposal.
    let cleanupEvidence = "CleanupUnknown"
    let cleanupError: String?
    let lastError: String?
}

/// Admission and callback ordering only. Go work is owned by the provider's serial queue.
final class TunnelLifecycle {
    private let lock = NSLock()
    private var generation: UInt64 = 0
    private var identity: TunnelIdentity?
    private var phase = "idle"
    private var stopPending = false
    private var quarantined = false
    private var runtimeStopped: Bool?
    private var cleanupError: String?
    private var lastError: String?
    private var operationRequestID: String?
    private var uncertainSettingsGeneration: UInt64?

    func start(_ identity: TunnelIdentity) throws -> UInt64 {
        lock.lock(); defer { lock.unlock() }
        guard !quarantined, !stopPending, phase == "idle" || phase == "stopped" else {
            throw lifecycleError("A tunnel operation is active or uncertain; a new core cannot be started")
        }
        generation += 1
        self.identity = identity
        operationRequestID = identity.requestID
        phase = "starting"
        runtimeStopped = false
        lastError = nil
        return generation
    }

    func stop() -> UInt64? {
        lock.lock(); defer { lock.unlock() }
        guard !stopPending else { return nil }
        generation += 1 // invalidates any start/settings/reload receipt immediately
        stopPending = true
        phase = "stopping"
        return generation
    }

    func reload() throws -> UInt64 {
        lock.lock(); defer { lock.unlock() }
        guard !quarantined, !stopPending, phase == "running" else {
            throw lifecycleError("The tunnel cannot accept reload while a lifecycle operation is active or uncertain")
        }
        generation += 1
        operationRequestID = "rpc-reload:" + UUID().uuidString
        phase = "reloadAccepted"
        return generation
    }

    func prepareStop(identity: TunnelIdentity, requestID: String) -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard self.identity == identity, !requestID.isEmpty, !stopPending else { return false }
        operationRequestID = requestID
        return true
    }

    func currentGeneration() -> UInt64 { lock.lock(); defer { lock.unlock() }; return generation }

    func accepts(_ token: UInt64) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return generation == token && !quarantined && !stopPending
    }

    @discardableResult
    func complete(_ token: UInt64, phase: String, stopped: Bool?, error: String? = nil, cleanup: String? = nil) -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard generation == token else { return false }
        if phase == "stopped" { stopPending = false }
        self.phase = quarantined ? "uncertain" : phase
        runtimeStopped = stopped
        if let error { lastError = error }
        if let cleanup { cleanupError = cleanup }
        return true
    }

    @discardableResult
    func timeout(_ token: UInt64, _ message: String, expectedPhase: String? = nil) -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard generation == token, expectedPhase == nil || phase == expectedPhase else { return false }
        quarantined = true
        phase = "uncertain"
        runtimeStopped = nil
        lastError = message
        return true
    }

    /// Once NE accepted a settings request, its OS effect can outlive the
    /// submitting generation. A superseding stop cannot dismiss that uncertainty.
    func settingsUncertain(_ submittedGeneration: UInt64, _ message: String) {
        lock.lock(); defer { lock.unlock() }
        quarantined = true
        phase = "uncertain"
        runtimeStopped = nil
        uncertainSettingsGeneration = submittedGeneration
        lastError = message
    }

    func retainCleanupFailure(_ error: String) {
        lock.lock(); defer { lock.unlock() }
        cleanupError = error
        quarantined = true
    }

    func report() -> TunnelReport {
        lock.lock(); defer { lock.unlock() }
        return TunnelReport(identity: identity, generation: generation, lifecycle: phase,
                            runtimeStopped: runtimeStopped, operationRequestID: operationRequestID,
                            uncertainSettingsGeneration: uncertainSettingsGeneration,
                            cleanupError: cleanupError, lastError: lastError)
    }
}

/// Watchdogs and normal callbacks race, but only one may complete a NE request.
final class TunnelCompletion<Value> {
    private let lock = NSLock()
    private var callback: ((Value) -> Void)?
    init(_ callback: @escaping (Value) -> Void) { self.callback = callback }
    @discardableResult
    func finish(_ value: Value, before: (() -> Void)? = nil) -> Bool {
        lock.lock()
        let callback = self.callback
        self.callback = nil
        lock.unlock()
        if callback != nil { before?() }
        callback?(value)
        return callback != nil
    }
}

/// A timed out network-settings callback cannot publish into a later session.
final class TunnelSettingsCompletion {
    private let lock = NSLock()
    private let done = DispatchSemaphore(value: 0)
    private var pending = true
    private var error: Error?
    @discardableResult
    func finish(_ error: Error?) -> Bool {
        lock.lock()
        guard pending else { lock.unlock(); return false }
        pending = false
        self.error = error
        lock.unlock()
        done.signal()
        return true
    }
    func wait(seconds: Double) throws {
        guard done.wait(timeout: .now() + seconds) == .success else {
            lock.lock(); pending = false; lock.unlock()
            throw TunnelSettingsTimeout()
        }
        lock.lock(); let error = self.error; lock.unlock()
        if let error { throw error }
    }
}

struct TunnelSettingsTimeout: LocalizedError {
    var errorDescription: String? { "Applying tunnel settings timed out; system settings may still change" }
}

private func lifecycleError(_ message: String) -> NSError {
    NSError(domain: "PolarisPacketTunnel", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
}
