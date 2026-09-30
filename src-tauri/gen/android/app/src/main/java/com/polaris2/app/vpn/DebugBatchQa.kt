package com.polaris2.app.vpn

import android.app.Activity
import android.app.KeyguardManager
import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Build
import android.os.PowerManager
import android.os.Process
import android.os.SystemClock
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import com.polaris2.app.BuildConfig
import java.io.ByteArrayOutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.SocketTimeoutException
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONArray
import org.json.JSONObject

/** Debug-only production adapter. No app_process, exported component, JNI factory or policy writes. */
internal object DebugBatchQa {
    private val random by lazy { SecureRandom() }
    private fun id(bytes: Int) = ByteArray(bytes).also(random::nextBytes).let(DebugBatchProtocol::hex)
    private val bootNonce by lazy { id(16) }
    private val gate = Any()
    private var active: Session? = null
    private var lastClosed: Session? = null // one bounded original-nonce tombstone, never re-armed
    private data class Binding(val runId: String, val birthNonce: String, val revision: Long, val digest: String)

    private class Session(val planSha: String, val apkSha: String, val expectedSourcePin: String,
                          val peers: List<String>, val port: Int, val deadline: Long,
                          val secret: ByteArray, val binding: Binding) {
        val appBootNonce = bootNonce // captured at admission; metadata reporters never initialize RNG
        val sessionId = id(16)
        val nonce = id(24)
        val tcpInstance = id(16)
        val udpInstance = id(16)
        val tcp = DebugBatchCounter(planSha, sessionId, nonce, tcpInstance, "tcp", deadline)
        val udp = DebugBatchCounter(planSha, sessionId, nonce, udpInstance, "udp", deadline)
        val lease = DebugBatchLease()
        val io = Executors.newFixedThreadPool(2) { Thread(it, "polaris-qa-io") }
        val guardian = Executors.newSingleThreadScheduledExecutor { Thread(it, "polaris-qa-guardian") }
        @Volatile var controllerLeaseUntil = minOf(deadline, SystemClock.elapsedRealtime() + 10000)
        @Volatile var armed = false
        @Volatile var abortReason: String? = null
        @Volatile var tcpBound = false
        @Volatile var udpBound = false
        @Volatile var prepared = false // actual APK verification and owner observation have returned
        @Volatile var lastSnapshot: String? = null
        data class PlatformSignal(val sampledAt: Long, val allowed: Boolean, val bindingCurrent: Boolean)
        private val createdAt = SystemClock.elapsedRealtime()
        private val signalPending = AtomicBoolean(false)
        @Volatile private var signal: PlatformSignal? = null
        @Volatile private var signalTask: DebugBatchTask? = null
        fun platformReady(): Boolean = signal.let { DebugBatchGuard.platformReady(SystemClock.elapsedRealtime(), it?.sampledAt, it?.let { s -> s.allowed && s.bindingCurrent }) }
        fun bindingCurrent(): Boolean {
            val input = DebugAppliedInputs.witness.snapshot()
            val owner = MainKernelAttemptRegistry.ownerForDrain()?.attempt
            return input.stage == "NativeInputReturned" && input.startAcknowledged && input.revision == binding.revision &&
                input.runId == binding.runId && input.birthNonce == binding.birthNonce && input.configDigest == binding.digest &&
                owner?.runId == binding.runId && owner.birthNonce == binding.birthNonce && !owner.revoked
        }
        fun allowed() = armed && !lease.snapshot().sealed && SystemClock.elapsedRealtime() < deadline &&
                        SystemClock.elapsedRealtime() < controllerLeaseUntil && platformReady()
        fun close(reason: String) {
            abortReason = abortReason ?: reason
            tcp.seal(); udp.seal()
            signalTask?.rejectBeforeRun()
            lease.seal()
            io.shutdownNow().forEach { (it as DebugBatchTask).rejectBeforeRun() }; guardian.shutdownNow()
            // No join here: a guardian or I/O callback may be the caller. External
            // cleanupObserve reads actual executor termination and the exact ledger.
        }
        fun complete(): Boolean = completeFor(null)
        fun completeFor(reporter: DebugBatchLease.CommandTicket?): Boolean {
            val s = lease.snapshotFor(reporter)
            return s.sealed && s.workers == 0 && s.commands == 0 && s.acquiring == 0 && s.closing == 0 && s.handles == 0 &&
                   !s.closeFailed && io.isTerminated && guardian.isTerminated
        }
        fun submit(action: () -> Unit) {
            if (!lease.workerBorn()) return
            val task = DebugBatchTask(lease, action) { close("io-unavailable") }
            try { io.execute(task) }
            catch (_: Throwable) { task.rejectBeforeRun(); close("executor-rejected") }
        }
        fun guardTick(activity: Activity) {
            if (lease.snapshot().sealed) { close(abortReason ?: "session-revoked"); return }
            // The deadline owner never calls SDK/Binder or waits an observer. A
            // blocked/redacted SDK sample expires and aborts; its original lease
            // remains Unknown until that actual task returns.
            val now = SystemClock.elapsedRealtime()
            val previous = signal
            DebugBatchGuard.abortReason(now, createdAt, deadline, controllerLeaseUntil, previous?.bindingCurrent ?: true, previous?.sampledAt, previous?.allowed)
                ?.let { close(it); return }
            if (previous != null && now - previous.sampledAt < 1000 || !signalPending.compareAndSet(false, true)) return
            if (!lease.workerBorn()) { signalPending.set(false); return }
            val task = DebugBatchTask(lease, {
                try { samplePlatform(activity) } finally { signalPending.set(false) }
            }) { close("guard-observer-unavailable") }
            signalTask = task
            try { DebugBatchCommandExecutor.value.execute(task) }
            catch (_: Throwable) { task.rejectBeforeRun(); close("guard-observer-rejected") }
        }
        private fun samplePlatform(activity: Activity) {
            if (lease.snapshot().sealed) return
            // Stamp before every potentially blocked registry/SDK boundary, never after it.
            val sampledAt = SystemClock.elapsedRealtime()
            val current = bindingCurrent()
            val power = activity.getSystemService(Context.POWER_SERVICE) as PowerManager
            val keyguard = activity.getSystemService(Context.KEYGUARD_SERVICE) as KeyguardManager
            val resumed = (activity as? LifecycleOwner)?.lifecycle?.currentState?.isAtLeast(Lifecycle.State.RESUMED) == true
            val observed = PlatformSignal(sampledAt, power.isInteractive && !keyguard.isKeyguardLocked && resumed, current)
            if (!lease.snapshot().sealed) signal = observed
        }
        fun tcpLoop() {
            if (!lease.beginAcquire()) return
            val server = try { ServerSocket() } catch (e: Throwable) { lease.acquireFailed(); throw e }
            if (!lease.publish(server)) return
            try {
                server.reuseAddress = false
                server.bind(InetSocketAddress(InetAddress.getByAddress(byteArrayOf(127, 0, 0, 1)), port), 4)
                server.soTimeout = 250; tcpBound = true
                while (allowed()) {
                    if (!lease.beginAcquire()) break
                    val socket = try { server.accept() } catch (_: SocketTimeoutException) { lease.acquireFailed(); continue }
                        catch (error: Throwable) { lease.acquireFailed(); throw error }
                    if (!lease.publish(socket)) break
                    try {
                        socket.soTimeout = 1000
                        // One bounded authenticated exchange per accepted socket.
                        val payload = ByteArrayOutputStream()
                        val input = socket.getInputStream()
                        while (payload.size() <= DebugBatchProtocol.MAX_BYTES) {
                            val b = input.read(); if (b == -1 || b == 10) break; payload.write(b)
                        }
                        val bytes = payload.toByteArray()
                        val frame = DebugBatchProtocol.decode(secret, bytes)
                        if (allowed() && tcp.accept(frame, bytes.size, SystemClock.elapsedRealtime())) {
                            socket.getOutputStream().write(DebugBatchProtocol.encode(secret, checkNotNull(frame).copy(kind = "ACK")) + byteArrayOf(10))
                        }
                    } catch (_: SocketTimeoutException) { /* no reachability/policy conclusion */ }
                    finally { lease.retire(socket) }
                    if (tcp.snapshot().invalid >= 64) { close("invalid-input-budget"); break }
                }
            } finally { lease.retire(server) }
        }
        fun udpLoop() {
            if (!lease.beginAcquire()) return
            val socket = try { DatagramSocket(null) } catch (e: Throwable) { lease.acquireFailed(); throw e }
            if (!lease.publish(socket)) return
            try {
                socket.reuseAddress = false
                socket.bind(InetSocketAddress(InetAddress.getByAddress(byteArrayOf(127, 0, 0, 1)), port))
                socket.soTimeout = 250; udpBound = true
                while (allowed()) {
                    val packet = DatagramPacket(ByteArray(DebugBatchProtocol.MAX_BYTES + 1), DebugBatchProtocol.MAX_BYTES + 1)
                    try { socket.receive(packet) } catch (_: SocketTimeoutException) { continue }
                    val bytes = packet.data.copyOfRange(packet.offset, packet.offset + packet.length)
                    val frame = DebugBatchProtocol.decode(secret, bytes)
                    if (allowed() && udp.accept(frame, bytes.size, SystemClock.elapsedRealtime())) {
                        val ack = DebugBatchProtocol.encode(secret, checkNotNull(frame).copy(kind = "ACK"))
                        socket.send(DatagramPacket(ack, ack.size, packet.socketAddress))
                    }
                    if (udp.snapshot().invalid >= 64) { close("invalid-input-budget"); break }
                }
            } finally { lease.retire(socket) }
        }
    }

    /** Admit the immutable original session before the first shared-executor queue boundary. */
    fun commandTask(activity: Activity, action: String, sessionId: String?, plan: String?,
                    success: (String) -> Unit, failure: (Throwable) -> Unit): DebugBatchCommandTask {
        check(BuildConfig.DEBUG) { "Debug batch QA is disabled" }
        require(action in setOf("prepare", "arm", "snapshot", "probe", "health", "close", "cleanupObserve"))
        require(action.length <= 32 && (sessionId?.length ?: 0) <= 64 && (plan?.length ?: 0) <= 4096)
        val candidate = if (action == "prepare") provisionalSession(checkNotNull(plan)) else null
        val (session, ticket) = try {
            synchronized(gate) {
                val original = if (candidate != null) {
                    check(active == null || checkNotNull(active).complete()) { "another batch still owns resources" }
                    active?.let { it.secret.fill(0); lastClosed = it }
                    active = candidate
                    candidate
                } else active?.takeIf { it.sessionId == sessionId }
                    ?: if (action == "cleanupObserve") lastClosed?.takeIf { it.sessionId == sessionId } else null
                checkNotNull(original) { "session not owned by this app boot" }
                original to checkNotNull(original.lease.commandBorn(action)) { "session already sealed" }
            }
        } catch (error: Throwable) {
            candidate?.close("admission-rejected"); candidate?.secret?.fill(0); throw error
        }
        val task = DebugBatchCommandTask(session.lease, ticket, { originalTicket ->
            if (session.lease.snapshot().sealed && originalTicket.action !in setOf("close", "cleanupObserve"))
                error("original session was sealed before command entered")
            success(runCommand(activity, session, originalTicket))
        }) { error -> session.close("command-unavailable"); failure(error) }
        if (action == "prepare") {
            try { session.guardian.scheduleAtFixedRate({ try { session.guardTick(activity) } catch (_: Throwable) { session.close("guardian-failed") } }, 0, 250, TimeUnit.MILLISECONDS) }
            catch (_: Throwable) { session.close("guardian-rejected") }
        }
        return task
    }

    /** Direct internal callers use the identical custody path; Release rejects before any resource. */
    fun command(activity: Activity, action: String, sessionId: String?, plan: String?): String {
        check(BuildConfig.DEBUG) { "Debug batch QA is disabled" }
        var result: String? = null
        var error: Throwable? = null
        commandTask(activity, action, sessionId, plan, { result = it }, { error = it }).run()
        error?.let { throw it }
        return checkNotNull(result)
    }

    /** Both native bridge legs admit the immutable original probe ticket before queueing. */
    private fun privateProbeTask(sessionId: String, success: (String) -> Unit,
                                 failure: (Throwable) -> Unit,
                                 credential: ByteArray? = null,
                                 action: (Session, DebugBatchLease.CommandTicket) -> String): DebugBatchCommandTask {
        check(BuildConfig.DEBUG) { "Debug core probe loan is disabled" }
        require(sessionId.matches(Regex("[0-9a-f]{32}"))) { "Core probe session unavailable" }
        val (session, ticket) = synchronized(gate) {
            val original = checkNotNull(active?.takeIf { it.sessionId == sessionId }) { "Core probe session unavailable" }
            original to checkNotNull(original.lease.commandBorn("probe")) { "Core probe session sealed" }
        }
        val buffer = credential?.let(::DebugCoreProbeCredentialBuffer)
        val task = DebugBatchCommandTask(session.lease, ticket, { originalTicket ->
            try {
                check(session.prepared && session.allowed() && session.lease.ownsProbe(originalTicket)) { "Core probe session unavailable" }
                success(action(session, originalTicket))
            } finally { buffer?.let(session.lease::retire) }
        }) { error -> session.close("core-probe-unavailable"); failure(error) }
        try {
            // Original command custody includes its queued credential. Guardian seal erases
            // these bytes even if the shared command executor is blocked by another task.
            if (buffer != null) {
                check(session.lease.beginAcquire()) { "Core probe session sealed" }
                check(session.lease.publish(buffer)) { "Core probe session sealed" }
            }
        } catch (error: Throwable) { task.rejectBeforeRun(error); throw error }
        return task
    }

    /** Called before actual JNI input mutation and Stop seal; no I/O or native ownership here. */
    fun nativeInputChanged(owner: MainKernelAttempt<*>) {
        if (!BuildConfig.DEBUG) return
        val original = synchronized(gate) {
            active?.takeIf { it.binding.runId == owner.runId && it.binding.birthNonce == owner.birthNonce }
        } ?: return
        original.abortReason = original.abortReason ?: "native-input-changed"
        original.lease.revokeProbeCredentials()
    }

    private fun Session.probeScope(generation: String) = DebugCoreProbeLoan.Scope(
        appBootNonce, sessionId, nonce, planSha, apkSha, expectedSourcePin, generation,
        binding.runId, binding.birthNonce, binding.revision, binding.digest, deadline,
    )

    /** No SDK/transport or batch gate around the actual Main operationLock/witness read. */
    fun coreProbeScopeTask(sessionId: String, success: (String) -> Unit,
                           failure: (Throwable) -> Unit): DebugBatchCommandTask =
        privateProbeTask(sessionId, success, failure) { session, _ ->
            val actual = checkNotNull(DebugCoreProbeLoan.currentInput()) { "Core probe Main input unavailable" }.snapshot
            check(actual.runId == session.binding.runId && actual.birthNonce == session.binding.birthNonce &&
                actual.revision == session.binding.revision && actual.configDigest == session.binding.digest && session.allowed()) {
                "Core probe Main input changed"
            }
            val s = session.probeScope("") // Kotlin cannot invent a Rust lifecycle generation.
            JSONObject().put("bootNonce", s.bootNonce).put("sessionId", s.sessionId).put("nonce", s.nonce)
                .put("planSha256", s.planSha256).put("apkSha256", s.apkSha256).put("expectedSourcePin", s.expectedSourcePin)
                .put("runId", s.runId).put("birthNonce", s.birthNonce).put("revision", s.revision)
                .put("configDigest", s.configDigest).put("deadlineElapsed", s.deadlineElapsed)
                .put("sampledElapsed", SystemClock.elapsedRealtime()).toString()
        }

    /**
     * Source-only production admission: PC receiver ABI is not frozen, so erase the loan and
     * explicitly leave transport NotObserved. No socket is created or CONNECT attempted.
     */
    fun coreProbeLoanTask(args: DebugCoreProbeLoanArgs, success: (String) -> Unit,
                          failure: (Throwable) -> Unit): DebugBatchCommandTask {
        try {
            check(BuildConfig.DEBUG) { "Debug core probe loan is disabled" }
            val requested = DebugCoreProbeLoan.Scope(args.bootNonce, args.sessionId, args.nonce, args.planSha256,
                args.apkSha256, args.expectedSourcePin, args.generation, args.runId, args.birthNonce,
                args.revision, args.configDigest, args.deadlineElapsed)
            return privateProbeTask(args.sessionId, success, { error -> args.password.fill(0); failure(error) }, args.password) { session, ticket ->
                try {
                    val actualScope = session.probeScope(args.generation)
                    val loan = DebugCoreProbeLoan.admit(actualScope,
                        DebugCoreProbeLoan.Binding(requested, args.probePort, args.expiresElapsed),
                        DebugCoreProbeLoan.currentInput(), session.lease, ticket, SystemClock.elapsedRealtime(), args.password)
                    try {
                        check(session.allowed() && loan.isCurrent(SystemClock.elapsedRealtime(), DebugCoreProbeLoan.currentInput())) {
                            "Core probe input changed"
                        }
                    } finally { session.lease.retire(loan) }
                    metadataReport(session, ticket).let(::JSONObject)
                        .put("coreProbeLoan", "ActualStartBoundCredentialAdmittedAndErased")
                        .put("coreProbeTransport", "NotObserved")
                        .put("coreProbePath", "Unknown").toString()
                } finally { args.password.fill(0) }
            }
        } catch (failure: Throwable) {
            args.password.fill(0)
            throw failure
        }
    }

    /** No SDK/files/registry here: this provisional source cannot arm before actual verification. */
    private fun provisionalSession(plan: String): Session {
        val input = JSONObject(plan)
        require(input.keys().asSequence().toSet() == setOf("peers", "port", "ttlMillis", "apkSha256", "expectedSourcePin", "sessionSecret", "runId", "birthNonce", "nativeInputRevision"))
        val peers = input.getJSONArray("peers").let { a -> (0 until a.length()).map(a::getString) }
        val port = input.getInt("port"); val ttl = input.getLong("ttlMillis")
        DebugBatchProtocol.validatePlan(peers, port, ttl)
        val expectedSourcePin = input.getString("expectedSourcePin")
        val apkSha = input.getString("apkSha256")
        require(expectedSourcePin.matches(Regex("[0-9a-f]{64}")) && apkSha.matches(Regex("[0-9a-f]{64}")))
        val before = DebugAppliedInputs.witness.snapshot()
        val binding = Binding(input.getString("runId"), input.getString("birthNonce"), input.getLong("nativeInputRevision"), checkNotNull(before.configDigest))
        require(before.runId == binding.runId && before.birthNonce == binding.birthNonce && before.revision == binding.revision && before.stage == "NativeInputReturned" && before.startAcknowledged)
        val secret = DebugBatchProtocol.unhex(input.getString("sessionSecret"))
        val canonical = listOf(DebugBatchProtocol.SCHEMA, peers.sorted().joinToString(","), port.toString(), ttl.toString(), apkSha, expectedSourcePin, binding.runId, binding.birthNonce, binding.revision.toString(), binding.digest).joinToString("|")
        return Session(DebugBatchProtocol.sha(canonical), apkSha, expectedSourcePin, peers, port, SystemClock.elapsedRealtime() + ttl, secret, binding)
    }

    private fun runCommand(activity: Activity, session: Session, ticket: DebugBatchLease.CommandTicket): String {
        when (ticket.action) {
            "prepare" -> {
                val apkSha = activity.applicationInfo.sourceDir.let { filename ->
                    val digest = MessageDigest.getInstance("SHA-256")
                    java.io.File(filename).inputStream().use { stream -> val bytes = ByteArray(65536); while (true) { val n = stream.read(bytes); if (n < 0) break; digest.update(bytes, 0, n) } }
                    DebugBatchProtocol.hex(digest.digest())
                }
                check(apkSha == session.apkSha && session.bindingCurrent()) { "actual source differs" }
                synchronized(session) {
                    check(!session.lease.snapshot().sealed && SystemClock.elapsedRealtime() < session.deadline)
                    session.prepared = true
                }
            }
            "arm" -> {
                check(session.prepared && session.bindingCurrent() && session.platformReady() && SystemClock.elapsedRealtime() < session.deadline)
                synchronized(session) { check(!session.armed && !session.lease.snapshot().sealed); session.armed = true }
                session.submit(session::tcpLoop); session.submit(session::udpLoop)
            }
            "health" -> if (!session.lease.snapshot().sealed) session.controllerLeaseUntil = minOf(session.deadline, SystemClock.elapsedRealtime() + 10000)
            "close" -> session.close("controller-close")
        }
        // Only these two controls are metadata-only. Only cleanupObserve's exact
        // entered ticket may be excluded after this purely local report is formed.
        return if (ticket.action in setOf("close", "cleanupObserve")) metadataReport(session, ticket)
               else snapshotBody(activity, session).also { session.lastSnapshot = it }
    }

    /** Also protects a direct actual snapshot seam, independently of the plugin queue wrapper. */
    private fun snapshot(activity: Activity, session: Session): String {
        val ticket = checkNotNull(session.lease.commandBorn())
        check(session.lease.commandEntered(ticket))
        return try { snapshotBody(activity, session) } finally { session.lease.commandReturned(ticket) }
    }

    private fun metadataReport(session: Session, reporter: DebugBatchLease.CommandTicket): String {
        val report = session.lastSnapshot?.let(::JSONObject) ?: JSONObject()
            .put("schema", DebugBatchProtocol.SCHEMA).put("profile", "lan-inbound-app-witness")
            .put("source", if (session.prepared) "AppLive" else "AppUnverified")
            .put("bootNonce", session.appBootNonce).put("sessionId", session.sessionId).put("nonce", session.nonce)
            .put("planSha", session.planSha).put("apkSha256", if (session.prepared) session.apkSha else "Unknown")
            .put("expectedSourcePin", session.expectedSourcePin).put("input", JSONObject().put("stage", "Unknown"))
            .put("builderTunScope", JSONObject().put("source", "Unknown")).put("vpnUnderlying", "Unknown")
        fun counter(v: DebugBatchCounter.Snapshot) = JSONObject().put("matched", v.matched).put("health", v.health)
            .put("invalid", v.invalid).put("replay", v.replay).put("bytes", v.bytes).put("sealed", v.sealed)
        val lease = session.lease.snapshot()
        val others = session.lease.snapshotFor(reporter)
        report.put("tcp", counter(session.tcp.snapshot())).put("udp", counter(session.udp.snapshot()))
            .put("abortReason", session.abortReason).put("result", if (session.abortReason != null) "Aborted" else "PreparedOnly")
            .put("networkObservationStatus", if (session.lastSnapshot != null) "PreviouslyObservedReadOnly" else "Unknown")
            .put("networkSnapshotScope", "CachedReadOnlyMetadata")
            .put("cleanup", JSONObject().put("state", if (session.completeFor(reporter)) "OwnedWitnessResourcesReturned" else "Unknown")
                .put("workers", lease.workers).put("commands", lease.commands).put("commandsExcludingThisReporter", others.commands)
                .put("acquiring", lease.acquiring).put("closing", lease.closing).put("handles", lease.handles)
                .put("residualHandles", lease.residualHandles).put("closeFailed", lease.closeFailed)
                .put("ioTerminated", session.io.isTerminated).put("guardianTerminated", session.guardian.isTerminated)
                .put("observationScope", "OnlyExactMetadataCleanupObserveReporterExcluded")
                .put("artifacts", "ArtifactsNotCreated").put("independentOldSessionHealth", "NotObserved").put("mainNativeCleanup", "Unknown"))
        if (lease.sealed || !session.prepared) {
            report.getJSONObject("input").put("stage", "Unknown").put("startAcknowledged", false)
            report.put("builderTunScope", JSONObject().put("source", "Unknown"))
        }
        return report.toString()
    }

    private fun snapshotBody(activity: Activity, session: Session): String {
        val firstOwner = MainKernelAttemptRegistry.ownerForDrain()?.attempt
        val first = DebugAppliedInputs.witness.snapshot()
        val scope = firstOwner?.currentTunScope()
        val connectivity = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        val networks = runCatching { connectivity.allNetworks.take(16).map { network ->
            val caps = connectivity.getNetworkCapabilities(network)
            JSONObject().put("handleHash", DebugBatchProtocol.sha(network.networkHandle.toString()))
                .put("vpn", caps?.hasTransport(NetworkCapabilities.TRANSPORT_VPN))
                .put("wifi", caps?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI))
                .put("cellular", caps?.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR))
                .put("ownerUid", if (Build.VERSION.SDK_INT >= 30) caps?.ownerUid else null)
        } }.getOrNull()
        val second = DebugAppliedInputs.witness.snapshot()
        val secondOwner = MainKernelAttemptRegistry.ownerForDrain()?.attempt
        val stable = first == second && firstOwner === secondOwner && firstOwner?.revoked == false && session.bindingCurrent()
        fun counter(value: DebugBatchCounter.Snapshot) = JSONObject().put("matched", value.matched).put("health", value.health)
            .put("invalid", value.invalid).put("replay", value.replay).put("bytes", value.bytes).put("sealed", value.sealed)
        fun outcome(value: DebugBatchCounter.Snapshot) = if (value.matched > 0) "ObservedPass" else if (value.health > 0) "ObservedNoMatch" else "NotObserved"
        val lease = session.lease.snapshot()
        return JSONObject().put("schema", DebugBatchProtocol.SCHEMA).put("profile", "lan-inbound-app-witness")
            .put("source", "AppLive").put("bootNonce", session.appBootNonce).put("sessionId", session.sessionId).put("nonce", session.nonce)
            .put("planSha", session.planSha).put("apkSha256", session.apkSha).put("expectedSourcePin", session.expectedSourcePin)
            .put("packageName", activity.packageName).put("uid", Process.myUid()).put("pid", Process.myPid())
            .put("buildVersion", BuildConfig.VERSION_NAME).put("selinuxDomain", runCatching {
                java.io.File("/proc/self/attr/current").readText().trim().take(128)
            }.getOrDefault("Unknown"))
            .put("deadlineElapsedRealtime", session.deadline).put("tcpInstanceId", session.tcpInstance).put("udpInstanceId", session.udpInstance)
            .put("tcpBound", session.tcpBound).put("udpBound", session.udpBound).put("abortReason", session.abortReason)
            .put("tcp", counter(session.tcp.snapshot())).put("udp", counter(session.udp.snapshot()))
            .put("input", JSONObject().put("runId", first.runId).put("birthNonce", first.birthNonce).put("revision", first.revision)
                .put("configDigest", first.configDigest).put("stage", if (stable) first.stage else "Unknown")
                .put("startAcknowledged", stable && first.startAcknowledged).put("effectivePolicyReadback", "Unknown"))
            .put("builderTunScope", if (stable && scope != null) JSONObject().put("source", "BuilderAcceptedAndFdHandedOff")
                .put("autoRoute", scope.autoRoute).put("routeCount", scope.routes.size).put("excludedRouteCount", scope.excludedRoutes.size)
                .put("allowedPackageCount", scope.allowedPackages.size).put("excludedPackageCount", scope.excludedPackages.size)
                .put("skippedPackageCount", scope.skippedPackages.size).put("osUidRange", "Unknown") else JSONObject().put("source", "Unknown"))
            .put("networks", networks?.let(::JSONArray)).put("networkObservationStatus", if (networks == null) "Unknown" else "ObservedReadOnly")
            .put("networkSnapshotScope", "NonAtomicReadOnly").put("vpnUnderlying", "Unknown")
            .put("cleanup", JSONObject().put("state", if (session.complete()) "OwnedWitnessResourcesReturned" else "Unknown")
                .put("workers", lease.workers).put("commands", lease.commands).put("acquiring", lease.acquiring).put("closing", lease.closing)
                .put("handles", lease.handles).put("residualHandles", lease.residualHandles).put("closeFailed", lease.closeFailed).put("ioTerminated", session.io.isTerminated)
                .put("guardianTerminated", session.guardian.isTerminated).put("artifacts", "ArtifactsNotCreated")
                .put("independentOldSessionHealth", "NotObserved").put("mainNativeCleanup", "Unknown"))
            .put("cases", JSONObject().put("tcpWitness", outcome(session.tcp.snapshot())).put("udpAppSocket", outcome(session.udp.snapshot()))
                .put("witnessEvidenceScope", "AppSocketAuthenticatedRequestReceived").put("independentPeerAck", "NotObserved")
                .put("coreIngressAttribution", "Unknown").put("policyNegative", "NotObserved").put("tcpCoreOutbound", "NotObserved")
                .put("handover", "Deferred").put("dns", "Unknown").put("completeOsVpnScope", "Unknown"))
            .put("result", if (session.abortReason != null) "Aborted" else if (session.tcp.snapshot().matched + session.udp.snapshot().matched > 0) "ObservationsRecorded" else "PreparedOnly").toString()
    }
}
