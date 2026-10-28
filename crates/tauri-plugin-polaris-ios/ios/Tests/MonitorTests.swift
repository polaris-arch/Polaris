import Foundation

private final class Monitor {}

@main
struct MonitorTests {
    static func main() throws {
        let gate = TunnelMonitorSession<Monitor, String>()
        let a = try gate.publish("1", monitor: Monitor(), listener: "a", generation: 1).record
        assert(gate.beginDelivery(a) == "a")
        assert(gate.inFlight(a) == 1)
        let publication = try gate.publish("2", monitor: Monitor(), listener: "b", generation: 1)
        let b = publication.record
        assert(publication.previous === a)
        assert(gate.beginDelivery(a) == nil)
        let lateCloseA = try gate.close("1")
        assert(lateCloseA == nil) // Late Close(A) cannot cancel B.
        assert(gate.current() === b)
        gate.finishDelivery(a, observed: true) // The admitted A stack returns after B publication.
        assert(gate.inFlight(a) == 0)
        assert(!gate.hasObservation(a))
        assert(!gate.hasObservation(b))
        assert(gate.beginDelivery(b) == "b")
        gate.finishDelivery(b, observed: true)
        assert(gate.hasObservation(b))
        assert(b.first.wait(timeout: .now()) == .success)

        let duplicate = try gate.publish("2", monitor: Monitor(), listener: "another proxy", generation: 1)
        assert(!duplicate.inserted && duplicate.record === b)
        assert(gate.beginDelivery(b) == "b") // Synchronous callback may close itself without joining.
        let selfCloseB = try gate.close("2")
        assert(selfCloseB === b)
        assert(gate.inFlight(b) == 1)
        gate.finishDelivery(b, observed: true)
        assert(gate.inFlight(b) == 0 && !gate.hasObservation(b))

        _ = try gate.close("3") // Native revocation precedes a delayed Start.
        do {
            _ = try gate.publish("3", monitor: Monitor(), listener: "stale", generation: 1)
            assertionFailure("Close-before-start was forgotten")
        } catch {}
        for value in ["", "0", "-1", "01", "+1", " 4", "4 ", "9223372036854775808"] {
            do {
                _ = try gate.publish(value, monitor: Monitor(), listener: "invalid", generation: 1)
                assertionFailure("Invalid incarnation accepted")
            } catch {}
            do { _ = try gate.close(value); assertionFailure("Invalid close accepted") } catch {}
        }

        let timedOut = try gate.publish("4", monitor: Monitor(), listener: "timeout", generation: 2).record
        assert(timedOut.first.wait(timeout: .now()) == .timedOut)
        let timeoutClose = try gate.close("4")
        assert(timeoutClose === timedOut)
        let successor = try gate.publish("5", monitor: Monitor(), listener: "successor", generation: 2).record
        assert(gate.beginDelivery(timedOut) == nil)
        let lateTimeoutClose = try gate.close("4")
        assert(lateTimeoutClose == nil && gate.current() === successor)
        assert(gate.reset() === successor)
        assert(gate.current() == nil && gate.beginDelivery(successor) == nil)
        do {
            _ = try gate.publish("5", monitor: Monitor(), listener: "reset", generation: 2)
            assertionFailure("Reset allowed old incarnation")
        } catch {}
        let restarted = try gate.publish("6", monitor: Monitor(), listener: "restart", generation: 3).record
        assert(gate.beginDelivery(restarted) == "restart")
        gate.finishDelivery(restarted, observed: true)
        assert(gate.hasObservation(restarted))

        let registration = TunnelMonitorSession<Monitor, String>()
        let pending = try registration.publish("1", monitor: Monitor(), listener: "pending", generation: 1).record
        assert(registration.beginRegistration(pending))
        let closing = try registration.close("1")
        assert(closing === pending && registration.takeCancellation(pending) == nil)
        let next = try registration.publish("2", monitor: Monitor(), listener: "next", generation: 1).record
        assert(registration.finishRegistration(pending) === pending.monitor)
        assert(registration.takeCancellation(pending) == nil)
        assert(registration.current() === next)
        assert(registration.beginRegistration(next))
        assert(registration.finishRegistration(next) == nil)
        let reset = registration.reset()
        assert(reset === next && registration.takeCancellation(next) === next.monitor)
        assert(registration.takeCancellation(next) == nil)
        print("Monitor incarnation counterexamples passed; admission only, no SDK/core or exact cleanup evidence.")
    }
}
