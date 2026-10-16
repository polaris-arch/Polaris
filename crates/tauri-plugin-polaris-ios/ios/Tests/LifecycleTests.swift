import Foundation

@main
struct LifecycleTests {
    static func main() throws {
        try directoryRetirementCases()
        try reloadCustodyCases()
        managedBootstrapCases()
        ordinaryStopDispatchCases()
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
        var disconnectedStop = successfulStop
        disconnectedStop["active"] = false; disconnectedStop["running"] = false; disconnectedStop["status"] = 1
        disconnectedStop["extensionGeneration"] = NSNumber(value: 8)
        assert(vpnIdempotentStopIsConfirmed(evidence: disconnectedStop, expectedGeneration: 7))
        assert(!vpnIdempotentStopIsConfirmed(evidence: disconnectedStop, expectedGeneration: 8))
        assert(disconnectedStop["tailscaleStoreRetirement"] == nil)
        var staleDisconnectedStop = disconnectedStop
        staleDisconnectedStop["runtimeStopped"] = nil
        assert(!vpnIdempotentStopIsConfirmed(evidence: staleDisconnectedStop, expectedGeneration: 7))
        staleDisconnectedStop = disconnectedStop; staleDisconnectedStop["lastError"] = "unknown cleanup"
        assert(!vpnIdempotentStopIsConfirmed(evidence: staleDisconnectedStop, expectedGeneration: 7))
        // Actual recovery helper used by the disconnected no-op Stop branch.
        // A host deadline/read failure must yield to a late clean native report.
        var hostSession: String? = "session-a"
        var hostReason: String? = "Host Stop deadline expired"
        let oldOperation = report["operationRequestID"] as? String
        assert(vpnRecoverRepeatedStop(evidence: disconnectedStop, expectedGeneration: 7,
            uncertainSession: &hostSession, uncertainReason: &hostReason))
        assert(hostSession == nil && hostReason == nil)
        assert(vpnIdleFailure(evidence: disconnectedStop, profileExists: true, active: false) == nil)
        assert(report["operationRequestID"] as? String == oldOperation)
        assert(disconnectedStop["tailscaleStoreRetirement"] == nil)
        for key in ["cleanupError", "lastError", "uncertainSettingsGeneration"] {
            var bad = disconnectedStop
            if key == "uncertainSettingsGeneration" { bad[key] = NSNumber(value: 9) }
            else { bad[key] = "provider quarantine" }
            hostSession = "session-a"; hostReason = "host observation unknown"
            assert(!vpnRecoverRepeatedStop(evidence: bad, expectedGeneration: 7,
                uncertainSession: &hostSession, uncertainReason: &hostReason))
            assert(hostSession == "session-a" && hostReason == "host observation unknown")
        }
        hostSession = "different-session"; hostReason = "other owner still unknown"
        assert(!vpnRecoverRepeatedStop(evidence: disconnectedStop, expectedGeneration: 7,
            uncertainSession: &hostSession, uncertainReason: &hostReason))
        assert(hostSession == "different-session" && hostReason == "other owner still unknown")
        // Execute the failure transition used by VpnPlugin.stop's actual catch,
        // then retry A. A rejected Stop must not rebind uncertainty B to A.
        hostSession = "session-b"; hostReason = "late B callback remains unknown"
        for _ in 0..<2 {
            do {
                guard vpnRecoverRepeatedStop(evidence: disconnectedStop, expectedGeneration: 7,
                    uncertainSession: &hostSession, uncertainReason: &hostReason) else {
                    throw NSError(domain: "PolarisVPN", code: 1,
                        userInfo: [NSLocalizedDescriptionKey: "CleanupUnknown: The disconnected session has no matching completed Stop"])
                }
                assertionFailure("A cannot clear B's host uncertainty")
            } catch {
                vpnRecordStopFailure(sessionID: "session-a", reason: error.localizedDescription,
                    uncertainSession: &hostSession, uncertainReason: &hostReason)
            }
            assert(hostSession == "session-b" && hostReason == "late B callback remains unknown")
        }
        // A failure for its own scope can still be recorded and then recovered.
        hostSession = nil; hostReason = nil
        vpnRecordStopFailure(sessionID: "session-a", reason: "same-session host timeout",
            uncertainSession: &hostSession, uncertainReason: &hostReason)
        assert(vpnRecoverRepeatedStop(evidence: disconnectedStop, expectedGeneration: 7,
            uncertainSession: &hostSession, uncertainReason: &hostReason))
        assert(hostSession == nil && hostReason == nil)
        hostSession = nil; hostReason = "unscoped host failure"
        vpnRecordStopFailure(sessionID: "session-a", reason: "narrower failure",
            uncertainSession: &hostSession, uncertainReason: &hostReason)
        assert(hostSession == nil && hostReason == "unscoped host failure")
        var wrongProfile: [String: Any] = ["status":1, "running":false, "active":false]
        wrongProfile.merge(vpnSessionEvidence(profileIdentity: ["sessionID":"different", "requestID":"start-a", "configDigest":"digest-a"],
            report: report, stopRequestID: oldOperation)) { _, live in live }
        hostSession = "session-a"; hostReason = "host unknown"
        assert(!vpnRecoverRepeatedStop(evidence: wrongProfile, expectedGeneration: 7,
            uncertainSession: &hostSession, uncertainReason: &hostReason))
        assert(hostReason == "host unknown")
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
    static func directoryRetirementCases() throws {
        let digest = String(repeating: "a", count: 64)
        let identity = TunnelIdentity(sessionID: "directory-session", requestID: "directory-start", configDigest: digest)
        let profile: [String: Any] = ["sessionID": identity.sessionID, "requestID": identity.requestID, "configDigest": digest]
        let node: [String: Any] = ["tag": "ts-a", "stateDirectory": "/managed/ts-a", "directoryIdentity": "unix:1:2:1:3",
            "incarnation": String(repeating: "c", count: 32), "directoryEpoch": String(repeating: "e", count: 32), "writerState": "Unknown", "censusVersion": 1, "failure": ""]
        func export(_ nodes: [[String: Any]], terminal: String = "Unknown") throws -> String {
            let value: [String: Any] = ["contractVersion": "polaris-ts-directory-retirement-v1", "globalCleanupEvidence": "CleanupUnknown",
                "instances": [["runNonce": String(repeating: "b", count: 64), "configDigest": digest,
                "terminal": terminal, "censusComplete": true, "nodes": nodes]]]
            return String(data: try JSONSerialization.data(withJSONObject: value), encoding: .utf8)!
        }
        let original = try export([node])
        var drained = node; drained["writerState"] = "SealedDrained"
        let terminal = try export([drained], terminal: "SealedDrained")
        let lifecycle = TunnelLifecycle()
        let generation = try lifecycle.start(identity)
        lifecycle.retainTailscaleDirectoryScope(original, identity: identity)
        assert(lifecycle.complete(generation, phase: "running", stopped: false))
        var response: TunnelObservation?
        assert(lifecycle.prepareStop(identity: identity, requestID: "stop-a", nonce: "nonce-a",
            expectedGeneration: generation, observation: { response = $0 }))
        // Admission cannot replace the frozen census between prepareStop and stop.
        lifecycle.retainTailscaleDirectoryScope("late-live", identity: identity)
        let stopping = lifecycle.stop()!
        lifecycle.retainTailscaleDirectoryScope(terminal, identity: identity, terminal: true)
        lifecycle.retainTailscaleDirectoryScope("late-close", identity: identity, terminal: true)
        assert(lifecycle.complete(stopping, phase: "stopped", stopped: true))
        let observation = try JSONSerialization.jsonObject(with: JSONEncoder().encode(response!)) as! [String: Any]
        func evidence(_ observed: [String: Any], nonce: String = "nonce-a") -> [String: Any]? {
            vpnDirectoryRetirementEvidence(profileIdentity: profile, expectedIdentity: profile,
                observation: observed, nonce: nonce, stopRequestID: "stop-a", expectedGeneration: generation)
        }
        assert(evidence(observation)?["tailscaleDirectoryRetirement"] != nil)
        assert(evidence(observation)?["tailscaleStoreRetirement"] == nil)
        assert(evidence(observation, nonce: "replay") == nil)
        let report = observation["report"] as! [String: Any]
        assert(report["stopOriginalDirectoryScope"] as? String == original)
        assert(report["tailscaleDirectoryScope"] as? String == terminal)
        assert(vpnSessionEvidence(profileIdentity: profile, report: report)["tailscaleDirectoryRetirement"] == nil)
        for (field, replacement) in [("incarnation", String(repeating: "d", count: 32)),
                                     ("directoryIdentity", "unix:1:2:1:4"), ("directoryEpoch", String(repeating: "f", count: 32)),
                                     ("writerState", "Unknown"), ("failure", "timeout")] {
            var changed = drained; changed[field] = replacement
            var badReport = report; badReport["tailscaleDirectoryScope"] = try export([changed], terminal: "SealedDrained")
            var bad = observation; bad["report"] = badReport
            assert(evidence(bad) == nil)
        }
        for field in ["stopOriginalDirectoryScope", "tailscaleDirectoryScope"] {
            var badReport = report; badReport.removeValue(forKey: field)
            var bad = observation; bad["report"] = badReport
            assert(evidence(bad) == nil)
        }
        var badReport = report; badReport["lifecycle"] = "uncertain"
        var bad = observation; bad["report"] = badReport
        assert(evidence(bad) == nil)
        var missingEpoch = drained; missingEpoch.removeValue(forKey: "directoryEpoch")
        let missingEpochExport = try export([missingEpoch], terminal: "SealedDrained")
        assert(vpnTailscaleDirectoryExport(raw: missingEpochExport, configDigest: digest, terminal: true) == nil)
        var multiObservation = observation
        var multiReport = report
        for key in ["stopOriginalDirectoryScope", "tailscaleDirectoryScope"] {
            var value = try JSONSerialization.jsonObject(with: Data((report[key] as! String).utf8)) as! [String: Any]
            var runs = value["instances"] as! [[String: Any]]
            var old = runs[0]; old["runNonce"] = String(repeating: "f", count: 64)
            old["configDigest"] = String(repeating: "d", count: 64)
            runs.insert(old, at: 0); value["instances"] = runs
            multiReport[key] = String(data: try JSONSerialization.data(withJSONObject: value), encoding: .utf8)!
        }
        multiObservation["report"] = multiReport
        assert(evidence(multiObservation) != nil)
        let unknown = TunnelLifecycle()
        let first = try unknown.start(identity)
        unknown.retainTailscaleDirectoryScope(original, identity: identity)
        assert(unknown.complete(first, phase: "running", stopped: false))
        assert(unknown.prepareStop(identity: identity, requestID: "stop-b", nonce: "nonce-b", observation: { _ in }))
        let final = unknown.stop()!
        unknown.retainTailscaleDirectoryScope(original, identity: identity, terminal: true)
        unknown.retainTailscaleDirectoryScope(terminal, identity: identity, terminal: true)
        assert(unknown.complete(final, phase: "stopped", stopped: true))
        assert(unknown.report().tailscaleDirectoryScope == original)
    }

    static func reloadCustodyCases() throws {
        let digest = String(repeating: "a", count: 64)
        let profile: [String: Any] = ["sessionID": "reload-session", "requestID": "reload-start", "configDigest": digest]
        let node: [String: Any] = ["tag": "ts-a", "stateDirectory": "/managed/ts-a", "stateFile": "/managed/ts-a/tailscaled.state",
            "writerState": "Unknown", "stateFileState": "Unknown", "stateFileRevision": "", "profileState": "Unknown", "profileFingerprint": ""]
        let current: [String: Any] = ["runNonce": String(repeating: "b", count: 64), "configDigest": digest,
            "terminal": "Unknown", "censusComplete": false, "nodes": [node]]
        var historical = current
        historical["runNonce"] = String(repeating: "c", count: 64)
        historical["configDigest"] = String(repeating: "d", count: 64)
        var oldNode = node; oldNode["writerState"] = "SealedDrained"
        historical["nodes"] = [oldNode]; historical["terminal"] = "SealedDrained"; historical["censusComplete"] = true
        let export: [String: Any] = ["contractVersion": "polaris-ts-auth-writer-retirement-v1",
            "globalCleanupEvidence": "CleanupUnknown", "instances": [historical, current]]
        let raw = String(data: try JSONSerialization.data(withJSONObject: export), encoding: .utf8)!
        var report: [String: Any] = ["identity": profile, "generation": 2, "lifecycle": "running",
            "runtimeStopped": false, "cleanupEvidence": "CleanupUnknown", "tailscaleStoreScope": raw]
        var observation: [String: Any] = ["observationNonce": "reload-nonce", "report": report]
        // Existing strict observation still rejects stale generation. Only the
        // dedicated reload adapter may pass a candidate to Rust custody checks.
        assert(vpnLiveEvidence(profileIdentity: profile, expectedIdentity: profile,
            observation: observation, nonce: "reload-nonce", expectedGeneration: 1) == nil)
        let fresh = vpnLiveEvidence(profileIdentity: profile, expectedIdentity: profile,
            observation: observation, nonce: "reload-nonce")
        assert(fresh?["tailscaleStoreScope"] != nil)
        assert(fresh?["tailscaleStoreRetirement"] == nil)
        assert(vpnLiveEvidence(profileIdentity: profile, expectedIdentity: profile,
            observation: observation, nonce: "replayed") == nil)
        var wrong = profile; wrong["sessionID"] = "another-owner"
        report["identity"] = wrong; observation["report"] = report
        assert(vpnLiveEvidence(profileIdentity: profile, expectedIdentity: profile,
            observation: observation, nonce: "reload-nonce") == nil)
    }

    static func ordinaryStopDispatchCases() {
        let original: [String: Any] = ["sessionID": "a", "requestID": "start-a", "configDigest": String(repeating: "a", count: 64)]
        var submissions = 0
        // Same production dispatch predicate, independent of unavailable live
        // evidence: a blocked reload/timeout/wrong nonce cannot suppress Stop.
        for _ in ["reloadAccepted", "observation-timeout", "wrong-nonce"] {
            if vpnMaySubmitOrdinaryStop(expected: original, actual: original) { submissions += 1 }
        }
        assert(submissions == 3)
        var successor = original; successor["sessionID"] = "b"
        assert(!vpnMaySubmitOrdinaryStop(expected: original, actual: successor))
        successor = original; successor["requestID"] = "start-b"
        assert(!vpnMaySubmitOrdinaryStop(expected: original, actual: successor))
        successor = original; successor["configDigest"] = String(repeating: "b", count: 64)
        assert(!vpnMaySubmitOrdinaryStop(expected: original, actual: successor))
        assert(!vpnMaySubmitOrdinaryStop(expected: original, actual: nil))
    }

    static func managedBootstrapCases() {
        let identity: [String: Any] = ["sessionID": "s", "requestID": "r", "configDigest": String(repeating: "a", count: 64)]
        let source = String(repeating: "b", count: 40)
        let capability: [String: Any] = ["contractVersion": "polaris-ts-directory-capability-v1",
            "sourceABI": "polaris-g4-directory-v1", "sourceIdentity": source,
            "managedRoot": "/group/polaris/tailscale-g4-v1", "logicalRoot": "/group/polaris/tailscale",
            "ownerNonce": String(repeating: "c", count: 32), "participating": true]
        let observation: [String: Any] = ["contractVersion": "polaris-ios-g4-bootstrap-v1", "identity": identity,
            "extensionGeneration": 1, "observationNonce": "fresh", "capability": capability]
        func proof(_ candidate: [String: Any], nonce: String = "fresh") -> [String: Any]? {
            vpnManagedBootstrapCapability(observation: candidate, expectedIdentity: identity,
                nonce: nonce, expectedSource: source, root: "/group/polaris")
        }
        assert(proof(observation) != nil)
        // Both actual root resolvers return resolved URLs before exporting paths.
        // This source-only seam checks the resulting exact-root handshake: an
        // alias spelling cannot substitute for the canonical root even if the
        // OS would resolve both to the same inode. No state is created/moved here.
        let canonicalRoot = URL(fileURLWithPath: "/private/var/group/polaris", isDirectory: true).resolvingSymlinksInPath().path
        var canonicalCapability = capability
        canonicalCapability["managedRoot"] = canonicalRoot + "/tailscale-g4-v1"
        canonicalCapability["logicalRoot"] = canonicalRoot + "/tailscale"
        var canonicalObservation = observation; canonicalObservation["capability"] = canonicalCapability
        assert(vpnManagedBootstrapCapability(observation: canonicalObservation, expectedIdentity: identity,
            nonce: "fresh", expectedSource: source, root: canonicalRoot) != nil)
        assert(vpnManagedBootstrapCapability(observation: canonicalObservation, expectedIdentity: identity,
            nonce: "fresh", expectedSource: source, root: canonicalRoot + "/.") == nil)
        assert(proof(observation, nonce: "old") == nil)
        var zeroGeneration = observation; zeroGeneration["extensionGeneration"] = 0
        assert(proof(zeroGeneration) == nil)
        var missingCapability = observation; missingCapability.removeValue(forKey: "capability")
        assert(proof(missingCapability) == nil)
        for (field, replacement) in [("sourceABI", "legacy"), ("sourceIdentity", String(repeating: "d", count: 40)),
                                     ("ownerNonce", ""), ("managedRoot", "/other/tailscale-g4-v1"),
                                     ("logicalRoot", "/other/tailscale")] {
            var bad = observation; var inner = capability; inner[field] = replacement; bad["capability"] = inner
            assert(proof(bad) == nil)
        }
        var bad = observation; var inner = capability; inner["participating"] = false; bad["capability"] = inner
        assert(proof(bad) == nil)
        bad = observation; var wrongOwner = identity; wrongOwner["sessionID"] = "other"; bad["identity"] = wrongOwner
        assert(proof(bad) == nil)
        let managed = vpnStartOptions(sessionID: "s", requestID: "r", config: "secret target config", managed: true)
        assert(managed["configContent"] == nil)
        assert(managed.count == 3)
        assert(managed["directoryBootstrapVersion"] as? String == "polaris-ios-g4-bootstrap-v1")
        let legacy = vpnStartOptions(sessionID: "s", requestID: "r", config: "legacy config", managed: false)
        assert(legacy["configContent"] as? String == "legacy config")
        assert(legacy["directoryBootstrapVersion"] == nil)
    }

}
