import Foundation

@main
struct LifecycleTests {
    static func main() throws {
        let identity = TunnelIdentity(sessionID: "session-a", requestID: "start-a", configDigest: "digest-a")
        let lifecycle = TunnelLifecycle()
        let start = try lifecycle.start(identity)
        assert(lifecycle.complete(start, phase: "running", stopped: false))
        let reload1 = try lifecycle.reload()
        assert(lifecycle.complete(reload1, phase: "running", stopped: false))
        let reload2 = try lifecycle.reload()
        assert(reload1 != reload2)
        assert(!lifecycle.timeout(reload1, "old reload watchdog"))
        assert(lifecycle.report().lifecycle == "reloadAccepted")
        assert(lifecycle.complete(reload2, phase: "running", stopped: false))
        // This watchdog started at the same generation as the reload. Success
        // won before it acquired the lifecycle lock, so phase must also be checked.
        assert(!lifecycle.timeout(reload2, "same-token watchdog after success", expectedPhase: "reloadAccepted"))
        assert(lifecycle.report().lifecycle == "running")

        let lateSettings = TunnelSettingsCompletion()
        do { try lateSettings.wait(seconds: 0); assertionFailure("Expected settings timeout") } catch {}
        assert(!lateSettings.finish(nil))
        assert(lifecycle.timeout(reload2, "settings timeout: system callback may still apply"))
        assert(!lifecycle.accepts(reload2))
        let stop = lifecycle.stop()!
        assert(!lifecycle.complete(start, phase: "running", stopped: false))
        assert(lifecycle.complete(stop, phase: "stopped", stopped: true))
        assert(lifecycle.report().runtimeStopped == true)
        assert(lifecycle.report().cleanupEvidence == "CleanupUnknown")
        assert(lifecycle.report().lifecycle == "uncertain")
        do { _ = try lifecycle.start(identity); assertionFailure("New core admitted after settings timeout") } catch {}

        // The OS settings request was submitted by start before stop superseded
        // it. It times out before stop's own deadline, then the Go stop returns.
        // Discarding this old token's timeout would admit a new core while the
        // original OS request can still apply settings to that new session.
        let supersededSettings = TunnelLifecycle()
        let submittingStart = try supersededSettings.start(identity)
        let pendingSystemCallback = TunnelSettingsCompletion()
        let supersedingStop = supersededSettings.stop()!
        assert(!supersededSettings.accepts(submittingStart))
        do {
            try pendingSystemCallback.wait(seconds: 0)
            assertionFailure("Expected old submitted settings request to time out")
        } catch {
            supersededSettings.settingsUncertain(submittingStart, error.localizedDescription)
        }
        assert(supersededSettings.complete(supersedingStop, phase: "stopped", stopped: true))
        assert(supersededSettings.report().runtimeStopped == true)
        assert(supersededSettings.report().uncertainSettingsGeneration == submittingStart)
        assert(supersededSettings.report().lifecycle == "uncertain")
        assert(!pendingSystemCallback.finish(nil))
        do { _ = try supersededSettings.start(identity); assertionFailure("New core admitted while old OS settings may still apply") } catch {}

        let heldCloseFailure = TunnelLifecycle()
        let failed = try heldCloseFailure.start(identity)
        heldCloseFailure.retainCleanupFailure("endpoint Close failed")
        let failedStop = heldCloseFailure.stop()!
        assert(heldCloseFailure.complete(failedStop, phase: "stopped", stopped: true))
        assert(!heldCloseFailure.complete(failed, phase: "running", stopped: false))
        assert(heldCloseFailure.report().cleanupError == "endpoint Close failed")
        assert(heldCloseFailure.report().cleanupEvidence == "CleanupUnknown")
        do { _ = try heldCloseFailure.start(identity); assertionFailure("New core admitted after close failure") } catch {}

        var calls = 0
        let callsLock = NSLock()
        let once = TunnelCompletion<Int> { _ in callsLock.lock(); calls += 1; callsLock.unlock() }
        DispatchQueue.concurrentPerform(iterations: 100) { once.finish($0) }
        assert(calls == 1)

        let profile: [String: Any] = ["sessionID": "session-a", "requestID": "start-a", "configDigest": "digest-a"]
        var report: [String: Any] = ["identity": profile, "operationRequestID": "old-stop", "runtimeStopped": true,
                                   "lifecycle": "stopped", "cleanupEvidence": "Exact", "cleanupError": "close failed"]
        // A cold host may address its profile but cannot classify it as an orphan.
        let cold = vpnSessionEvidence(profileIdentity: profile, report: nil)
        assert(cold["ownership"] as? String == "profileMatchedUnattested")
        assert(cold["runtimeStopped"] == nil)
        assert(vpnIdleFailure(evidence: cold, profileExists: true, active: false) != nil)
        assert(vpnIdleFailure(evidence: [:], profileExists: false, active: false) == nil)
        let legacy = vpnSessionEvidence(profileIdentity: nil, report: nil)
        assert(vpnIdleFailure(evidence: legacy, profileExists: true, active: false) != nil)
        let oldStop = vpnSessionEvidence(profileIdentity: profile, report: report, stopRequestID: "new-stop")
        assert(oldStop["runtimeStopped"] == nil)
        assert(oldStop["cleanupError"] as? String == "close failed")
        assert(oldStop["cleanupEvidence"] as? String == "CleanupUnknown")
        assert(vpnStopFailure(evidence: oldStop, profileExists: true) != nil)
        report["operationRequestID"] = "new-stop"
        let currentStop = vpnSessionEvidence(profileIdentity: profile, report: report, stopRequestID: "new-stop")
        assert(currentStop["runtimeStopped"] as? Bool == true)
        assert(currentStop["cleanupEvidence"] as? String == "CleanupUnknown")
        assert(vpnStopFailure(evidence: currentStop, profileExists: true) != nil) // matching Close failure still fails
        report["cleanupError"] = nil
        let successfulStop = vpnSessionEvidence(profileIdentity: profile, report: report, stopRequestID: "new-stop")
        assert(vpnStopFailure(evidence: successfulStop, profileExists: true) == nil)
        assert(vpnIdleFailure(evidence: successfulStop, profileExists: true, active: false) == nil)
        var incompleteStop = successfulStop
        incompleteStop["runtimeStopped"] = false
        assert(vpnStopFailure(evidence: incompleteStop, profileExists: true) != nil)
        incompleteStop["runtimeStopped"] = nil
        assert(vpnStopFailure(evidence: incompleteStop, profileExists: true) != nil)
        assert(vpnStopFailure(evidence: [:], profileExists: false) == nil)
        // Both host processes begin at generation 1. Their NE stop request IDs
        // must differ, and the old report must not acknowledge the new request.
        let oldHostRequest = freshVpnRequestID("1")
        let newHostRequest = freshVpnRequestID("1")
        assert(oldHostRequest != newHostRequest)
        report["operationRequestID"] = oldHostRequest
        let newHostStop = vpnSessionEvidence(profileIdentity: profile, report: report, stopRequestID: newHostRequest)
        assert(newHostStop["runtimeStopped"] == nil)
        assert(vpnStopFailure(evidence: newHostStop, profileExists: true) != nil)
        assert(vpnIdleFailure(evidence: newHostStop, profileExists: true, active: false) != nil)

        // A timeout cannot undo the OS preference write. Admission stays blocked
        // until that specific callback returns, then late callbacks cannot clear
        // a newer write or publish a new operation's success.
        let mutation = VpnPreferenceMutation()
        assert(mutation.begin(oldHostRequest))
        assert(mutation.pending)
        assert(!mutation.begin(newHostRequest))
        assert(!mutation.finish(newHostRequest))
        assert(mutation.pending)
        assert(mutation.finish(oldHostRequest))
        assert(!mutation.pending)
        assert(mutation.begin(newHostRequest))
        assert(!mutation.finish(oldHostRequest))
        assert(mutation.requestID == newHostRequest)
        var replaced = profile
        replaced["sessionID"] = "session-b"
        let wrongSession = vpnSessionEvidence(profileIdentity: replaced, report: report)
        assert(wrongSession["runtimeStopped"] == nil)
        assert(wrongSession["ownership"] as? String == "profileMatchedUnattested")

        let live = TunnelLifecycle()
        let liveGeneration = try live.start(identity)
        assert(live.complete(liveGeneration, phase: "running", stopped: false))
        let observed = live.observeSession(identity: identity, nonce: "fresh-nonce", expectedGeneration: liveGeneration)!
        let observation = try JSONSerialization.jsonObject(with: JSONEncoder().encode(observed)) as! [String: Any]
        let liveEvidence = vpnLiveEvidence(profileIdentity: profile, expectedIdentity: profile,
            observation: observation, nonce: "fresh-nonce", expectedGeneration: liveGeneration)!
        assert(liveEvidence["extensionGeneration"] as? NSNumber == NSNumber(value: liveGeneration))
        assert(liveEvidence["cleanupEvidence"] as? String == "CleanupUnknown")
        assert(vpnLiveEvidence(profileIdentity: replaced, expectedIdentity: profile, observation: observation,
            nonce: "fresh-nonce", expectedGeneration: liveGeneration) == nil)
        assert(vpnLiveEvidence(profileIdentity: profile, expectedIdentity: profile, observation: observation,
            nonce: "stale-nonce", expectedGeneration: liveGeneration) == nil)
        assert(vpnLiveEvidence(profileIdentity: profile, expectedIdentity: profile, observation: observation,
            nonce: "fresh-nonce", expectedGeneration: liveGeneration + 1) == nil)
        let other = TunnelIdentity(sessionID: "session-b", requestID: "start-b", configDigest: "digest-b")
        assert(live.observeSession(identity: other, nonce: "fresh-nonce") == nil)
        assert(live.observeSession(identity: identity, nonce: "") == nil)
        let changedGeneration = try live.reload()
        assert(live.observeSession(identity: identity, nonce: "fresh-nonce", expectedGeneration: liveGeneration) == nil)
        assert(live.complete(changedGeneration, phase: "running", stopped: false))
        live.settingsUncertain(changedGeneration, "late system settings")
        let uncertainObservation = try JSONSerialization.jsonObject(with: JSONEncoder().encode(
            live.observeSession(identity: identity, nonce: "fresh-nonce")!)) as! [String: Any]
        assert(vpnLiveEvidence(profileIdentity: profile, expectedIdentity: profile,
            observation: uncertainObservation, nonce: "fresh-nonce") == nil)

        // Actual JSON serialization of first-read (no generation yet) and bound
        // reads. Optional values must never be boxed into the provider message.
        let firstMessage = vpnObservationMessage(identity: profile, nonce: "first")
        let firstWire = try JSONSerialization.jsonObject(with: JSONSerialization.data(withJSONObject: firstMessage)) as! [String: Any]
        assert(firstWire["extensionGeneration"] == nil)
        let boundMessage = vpnObservationMessage(identity: profile, nonce: "bound", expectedGeneration: 41)
        let boundWire = try JSONSerialization.jsonObject(with: JSONSerialization.data(withJSONObject: boundMessage)) as! [String: Any]
        assert((boundWire["extensionGeneration"] as? NSNumber)?.uint64Value == 41)

        // These are the admission and continuation predicates used at the four
        // real NE boundaries: load, save, reload, and start already submitted.
        for boundary in ["load", "save", "reload", "submitted"] {
            let admission = VpnStartAdmission()
            let intent = admission.admit("start-" + boundary)!
            let write = VpnPreferenceMutation()
            if boundary == "save" { assert(write.begin(intent.requestID)) }
            if boundary == "submitted" { intent.submitted(profile) }
            admission.revoke(intent.requestID, pending: intent)
            assert(!intent.allowsContinuation) // Late preference callbacks cannot start.
            assert(intent.finish())
            assert(!intent.finish()) // Both timeout and late callback are terminal once.
            assert(!intent.mayStop(replaced)) // A late stop may never address B.
            assert(intent.mayStop(profile) == (boundary == "submitted"))
            if boundary == "save" {
                assert(write.pending) // Revocation cannot erase an unknown OS write.
                assert(!write.finish("start-b"))
                assert(write.finish(intent.requestID)) // Late save still reconciles A.
            }
        }
        let admission = VpnStartAdmission()
        admission.revoke("start-a", pending: nil)
        assert(admission.admit("start-a") == nil) // Stop dispatch can precede Start.
        assert(admission.admit("start-a") == nil) // Even a repeated late dispatch stays revoked.
        let bIntent = admission.admit("start-b")!
        admission.revoke("start-a", pending: bIntent)
        assert(bIntent.allowsContinuation)
        let readyBeforeRevoke = VpnStartIntent("ready-a")
        readyBeforeRevoke.submitted(profile)
        assert(readyBeforeRevoke.finish())
        admission.revoke("ready-a", pending: readyBeforeRevoke)
        assert(readyBeforeRevoke.revoked)
        assert(readyBeforeRevoke.mayStop(profile)) // Ready callback may precede revoke dispatch.
        assert(!readyBeforeRevoke.mayStop(replaced))
        for index in 0...64 { admission.revoke("never-dispatched-\(index)", pending: nil) }
        assert(admission.admit("never-dispatched-0") == nil)
        assert(admission.admit("new-after-capacity") == nil) // Fail closed, no unsafe eviction.

        assert(vpnForegroundFailure(appActive: true, foregroundScene: true) == nil)
        assert(vpnForegroundFailure(appActive: false, foregroundScene: true)?.hasPrefix("ForegroundRequired:") == true)
        assert(vpnForegroundFailure(appActive: true, foregroundScene: false)?.hasPrefix("ForegroundRequired:") == true)
        let denied = NSError(domain: NSPOSIXErrorDomain, code: 1)
        assert(vpnStartFailure(denied, stage: "save").hasPrefix("PermissionDenied:"))
        let cancelledPermit = NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError)
        assert(vpnStartFailure(cancelledPermit, stage: "save").hasPrefix("PermissionDenied:"))
        let unknownSave = NSError(domain: "NEVPNErrorDomain", code: 5)
        assert(vpnStartFailure(unknownSave, stage: "save").hasPrefix("StartupFailed:"))
        assert(vpnStartFailure(denied, stage: "load").hasPrefix("StartupFailed:"))
        let timeout = NSError(domain: "PolarisVPN", code: 1, userInfo: [NSLocalizedDescriptionKey: "timed out"])
        assert(vpnStartFailure(timeout, stage: "save").hasPrefix("StartupFailed:"))
        try storeRetirementCases()
        print("iOS lifecycle counterexamples passed: exact live observation, permission attribution, foreground, four pending-start revoke boundaries, late Stop(A)/Start(B), durable preference uncertainty, receipt ordering, sticky close failure, CleanupUnknown")
    }

    static func storeRetirementCases() throws {
        let digest = String(repeating: "d", count: 64)
        let profile: [String: Any] = ["sessionID": "scope-session", "requestID": "scope-start", "configDigest": digest]
        let identity = TunnelIdentity(sessionID: "scope-session", requestID: "scope-start", configDigest: digest)
        let node: [String: Any] = ["tag": "ts-a", "stateDirectory": "/private/group/ts-a",
            "stateFile": "/private/group/ts-a/tailscaled.state", "writerState": "SealedDrained",
            "stateFileState": "Regular", "stateFileRevision": String(repeating: "a", count: 64),
            "profileState": "Bound", "profileFingerprint": String(repeating: "b", count: 64)]
        let first: [String: Any] = ["runNonce": String(repeating: "1", count: 64), "configDigest": digest,
            "terminal": "SealedDrained", "censusComplete": true, "nodes": [node]]
        let second: [String: Any] = ["runNonce": String(repeating: "2", count: 64), "configDigest": digest,
            "terminal": "NoStoreConstruction", "censusComplete": true, "nodes": [[String: Any]]()]
        let export: [String: Any] = ["contractVersion": "polaris-ts-auth-writer-retirement-v1",
            "globalCleanupEvidence": "CleanupUnknown", "instances": [first, second]]
        func raw(_ value: [String: Any]) throws -> String {
            String(decoding: try JSONSerialization.data(withJSONObject: value), as: UTF8.self)
        }
        let lifecycle = TunnelLifecycle()
        let start = try lifecycle.start(identity)
        assert(lifecycle.complete(start, phase: "running", stopped: false))
        var received: TunnelObservation?
        assert(!lifecycle.prepareStop(identity: identity, requestID: "scope-stop", nonce: "scope-nonce",
            expectedGeneration: start + 1, observation: { received = $0 }))
        assert(!lifecycle.prepareStop(identity: identity, requestID: "scope-stop", nonce: "",
            observation: { received = $0 }))
        assert(lifecycle.prepareStop(identity: identity, requestID: "scope-stop", nonce: "scope-nonce",
            expectedGeneration: start, observation: {
                received = $0
                assert(lifecycle.report().generation == $0.report.generation) // Callback runs outside the original lock.
            }))
        assert(received == nil) // prepareStop is not terminal proof or immediate success.
        assert(!lifecycle.prepareStop(identity: identity, requestID: "other-stop", nonce: "other-nonce", observation: { received = $0 }))
        do { _ = try lifecycle.reload(); assertionFailure("Reload admitted between exact prepareStop and Stop") } catch {}
        let stop = lifecycle.stop()!
        lifecycle.retainTailscaleStoreScope(try raw(export), identity: identity)
        assert(lifecycle.complete(stop, phase: "stopped", stopped: true))
        assert(received?.observationNonce == "scope-nonce")
        let observation = try JSONSerialization.jsonObject(with: JSONEncoder().encode(received!)) as! [String: Any]
        let result = vpnRetirementEvidence(profileIdentity: profile, expectedIdentity: profile,
            observation: observation, nonce: "scope-nonce", stopRequestID: "scope-stop", expectedGeneration: start)!
        let wire = result["tailscaleStoreRetirement"] as! [String: Any]
        assert((wire["storeRetirement"] as! [String: Any])["instances"] as? [[String: Any]] != nil)
        assert(((wire["storeRetirement"] as! [String: Any])["instances"] as! [[String: Any]]).count == 2)
        assert(result["cleanupEvidence"] as? String == "CleanupUnknown")
        for (nonce, request, generation) in [("old-nonce", "scope-stop", start), ("scope-nonce", "old-stop", start),
            ("scope-nonce", "scope-stop", start + 1)] {
            assert(vpnRetirementEvidence(profileIdentity: profile, expectedIdentity: profile, observation: observation,
                nonce: nonce, stopRequestID: request, expectedGeneration: generation) == nil)
        }
        var changed = profile; changed["sessionID"] = "other-session"
        assert(vpnRetirementEvidence(profileIdentity: changed, expectedIdentity: profile, observation: observation,
            nonce: "scope-nonce", stopRequestID: "scope-stop") == nil)
        let report = observation["report"] as! [String: Any]
        assert(vpnSessionEvidence(profileIdentity: profile, report: report, stopRequestID: "scope-stop")["tailscaleStoreRetirement"] == nil)
        for (field, value) in [("cleanupError", "Close failed"), ("lastError", "timed out"), ("lifecycle", "uncertain")] {
            var failed = report; failed[field] = value
            assert(vpnRetirementEvidence(profileIdentity: profile, expectedIdentity: profile,
                observation: ["observationNonce": "scope-nonce", "report": failed], nonce: "scope-nonce", stopRequestID: "scope-stop") == nil)
        }
        var badExports = [[String: Any]]()
        var empty = export; empty["instances"] = [[String: Any]](); badExports.append(empty)
        var duplicate = export; duplicate["instances"] = [first, first]; badExports.append(duplicate)
        var partial = first; partial["censusComplete"] = false
        var partialExport = export; partialExport["instances"] = [partial, second]; badExports.append(partialExport)
        for (key, value) in [("stateDirectory", "/private/../ts-a"), ("stateFile", "/wrong/tailscaled.state"),
            ("writerState", "Unknown"), ("stateFileRevision", "stale"), ("profileFingerprint", "wrong"), ("stateFileState", "Missing")] {
            var invalid = node; invalid[key] = value
            var instance = first; instance["nodes"] = [invalid]
            var candidate = export; candidate["instances"] = [instance, second]; badExports.append(candidate)
        }
        for candidate in badExports {
            var failed = report; failed["tailscaleStoreScope"] = try raw(candidate)
            assert(vpnTailscaleStoreExport(report: failed, configDigest: digest, terminal: true) == nil)
        }
        var activeNode = node
        activeNode["writerState"] = "Unknown"; activeNode["stateFileState"] = "Unknown"; activeNode["stateFileRevision"] = ""
        activeNode["profileState"] = "Unknown"; activeNode["profileFingerprint"] = ""
        var activeInstance = first; activeInstance["terminal"] = "Unknown"; activeInstance["censusComplete"] = false; activeInstance["nodes"] = [activeNode]
        var activeExport = export; activeExport["instances"] = [activeInstance]
        var activeReport = report; activeReport["tailscaleStoreScope"] = try raw(activeExport)
        assert(vpnTailscaleStoreExport(report: activeReport, configDigest: digest, terminal: false) != nil)
        assert(vpnTailscaleStoreExport(report: activeReport, configDigest: digest, terminal: true) == nil)
        let preSubmit = VpnStartIntent("permission-denied-start")
        assert(preSubmit.noStoreTerminal() == nil)
        assert(preSubmit.finish())
        assert(preSubmit.noStoreTerminal()?["requestID"] as? String == "permission-denied-start")
        assert(!preSubmit.allowsContinuation)
        let attempted = VpnStartIntent("throwing-submission")
        attempted.submitted(profile) // Must happen before crossing startVPNTunnel, including throws.
        assert(attempted.finish())
        assert(attempted.noStoreTerminal() == nil)
        print("iOS TS Store retirement seam cases passed: deferred original Stop, full two-run census, exact nonce/generation, canonical paths, active Unknown, old-file exclusion, actual non-submission intent")
    }
}
