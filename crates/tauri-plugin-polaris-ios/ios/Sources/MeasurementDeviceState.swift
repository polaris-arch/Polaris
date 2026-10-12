import Foundation

/// A missing or unsatisfied native path is unknown, never "unmetered".
/// This cache observes cost only; it does not claim VPN/base-interface identity.
final class MeasurementDeviceState {
    private let lock = NSLock()
    private var metered: Bool?

    func recordMetered(_ value: Bool?) {
        lock.lock(); defer { lock.unlock() }
        metered = value
    }

    func snapshot(powerSave: Bool) -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        var result: [String: Any] = ["powerSave": powerSave]
        if let metered { result["metered"] = metered }
        else { result["metered"] = NSNull() }
        return result
    }
}
