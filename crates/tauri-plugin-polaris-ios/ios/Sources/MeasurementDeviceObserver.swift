import Foundation
import Network
import UIKit

/// Passive App-side path cost observation. Never opens a connection or changes
/// network/VPN preferences; the extension retains its own interface monitor.
final class MeasurementDeviceObserver {
    private let state = MeasurementDeviceState()
    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "com.polaris.app.measurement-cost")

    init() {
        monitor.pathUpdateHandler = { [weak self] path in
            self?.state.recordMetered(path.status == .satisfied ? path.isExpensive : nil)
        }
        monitor.start(queue: queue)
    }

    deinit { monitor.cancel() }

    func snapshot() -> [String: Any] {
        dispatchPrecondition(condition: .onQueue(.main))
        return state.snapshot(powerSave: ProcessInfo.processInfo.isLowPowerModeEnabled,
                              appActive: UIApplication.shared.applicationState == .active)
    }
}
