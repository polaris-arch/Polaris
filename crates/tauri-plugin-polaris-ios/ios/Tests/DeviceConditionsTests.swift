import Foundation

@main
struct DeviceConditionsTests {
    static func main() {
        let state = MeasurementDeviceState()
        assert(state.snapshot(powerSave: false)["appActive"] as? Bool == false)
        assert(state.snapshot(powerSave: false, appActive: true)["appActive"] as? Bool == true)
        assert(state.snapshot(powerSave: false, appActive: false)["appActive"] as? Bool == false)
        // Activity is fresh input, never retained from the previous active sample.
        assert(state.snapshot(powerSave: false)["appActive"] as? Bool == false)
        assert(state.snapshot(powerSave: false)["metered"] is NSNull)
        assert(state.snapshot(powerSave: false)["powerSave"] as? Bool == false)
        state.recordMetered(true)
        assert(state.snapshot(powerSave: false)["metered"] as? Bool == true)
        state.recordMetered(false)
        assert(state.snapshot(powerSave: true)["metered"] as? Bool == false)
        assert(state.snapshot(powerSave: true)["powerSave"] as? Bool == true)
        state.recordMetered(nil)
        assert(state.snapshot(powerSave: true)["metered"] is NSNull)
        assert(state.snapshot(powerSave: true)["powerSave"] as? Bool == true)
    }
}
