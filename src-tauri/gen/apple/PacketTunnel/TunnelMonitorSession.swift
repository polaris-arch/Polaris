import Foundation

/// Native incarnation admission only. SDK cancellation and Go calls never hold this gate.
/// Sealing does not prove callback drain, resource reclamation, or exact cleanup.
final class TunnelMonitorSession<Monitor: AnyObject, Listener> {
    final class Record {
        let incarnation: Int64
        let monitor: Monitor
        let listener: Listener
        let generation: UInt64
        let first = DispatchSemaphore(value: 0)
        fileprivate var sealed = false
        fileprivate var observed = false
        fileprivate var delivering = 0
        fileprivate var registering = false
        fileprivate var registered = false
        fileprivate var cancellationIssued = false

        fileprivate init(_ incarnation: Int64, monitor: Monitor, listener: Listener, generation: UInt64) {
            self.incarnation = incarnation
            self.monitor = monitor
            self.listener = listener
            self.generation = generation
        }
    }

    private let lock = NSLock()
    private var active: Record?
    private var startedThrough: Int64 = 0
    private var closedThrough: Int64 = 0

    private func identity(_ value: String) throws -> Int64 {
        guard let number = Int64(value), number > 0, String(number) == value else {
            throw NSError(domain: "PolarisMonitor", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "Invalid native interface monitor incarnation"])
        }
        return number
    }

    func publish(_ value: String, monitor: Monitor, listener: Listener, generation: UInt64)
        throws -> (record: Record, previous: Record?, inserted: Bool) {
        let incarnation = try identity(value)
        lock.lock()
        if let current = active, current.incarnation == incarnation {
            lock.unlock()
            return (current, nil, false) // A new gomobile proxy is not a new monitor.
        }
        guard incarnation > startedThrough, incarnation > closedThrough else {
            lock.unlock()
            throw NSError(domain: "PolarisMonitor", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "Native interface monitor was revoked or superseded"])
        }
        let previous = active
        previous?.sealed = true
        let record = Record(incarnation, monitor: monitor, listener: listener, generation: generation)
        startedThrough = incarnation
        active = record
        lock.unlock()
        previous?.first.signal()
        return (record, previous, true)
    }

    func close(_ value: String) throws -> Record? {
        let incarnation = try identity(value)
        lock.lock()
        closedThrough = max(closedThrough, incarnation) // Close before a delayed Start is remembered.
        let previous = active?.incarnation == incarnation ? active : nil
        if let previous {
            previous.sealed = true
            active = nil
        }
        lock.unlock()
        previous?.first.signal()
        return previous
    }

    func reset() -> Record? {
        lock.lock()
        let previous = active
        previous?.sealed = true
        closedThrough = max(closedThrough, startedThrough)
        active = nil
        lock.unlock()
        previous?.first.signal()
        return previous
    }

    func current() -> Record? {
        lock.lock(); defer { lock.unlock() }
        return active
    }

    func beginDelivery(_ record: Record) -> Listener? {
        lock.lock(); defer { lock.unlock() }
        guard active === record, !record.sealed else { return nil }
        record.delivering += 1
        return record.listener
    }

    func beginRegistration(_ record: Record) -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard active === record, !record.sealed, !record.registering, !record.registered else { return false }
        record.registering = true
        return true
    }

    func finishRegistration(_ record: Record) -> Monitor? {
        lock.lock()
        precondition(record.registering)
        record.registering = false
        record.registered = true
        lock.unlock()
        return takeCancellation(record)
    }

    func takeCancellation(_ record: Record) -> Monitor? {
        lock.lock(); defer { lock.unlock() }
        // Close racing SDK Start leaves compensation to the actual Start return.
        guard record.sealed, record.registered, !record.registering, !record.cancellationIssued else { return nil }
        record.cancellationIssued = true
        return record.monitor
    }

    func finishDelivery(_ record: Record, observed: Bool) {
        lock.lock()
        precondition(record.delivering > 0)
        record.delivering -= 1
        let publish = observed && active === record && !record.sealed
        if publish { record.observed = true }
        lock.unlock()
        if publish { record.first.signal() }
    }

    func hasObservation(_ record: Record) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return active === record && !record.sealed && record.observed
    }

    func inFlight(_ record: Record) -> Int {
        lock.lock(); defer { lock.unlock() }
        return record.delivering
    }
}
