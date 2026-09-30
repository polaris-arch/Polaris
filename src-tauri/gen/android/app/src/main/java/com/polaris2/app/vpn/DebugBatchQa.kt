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
        data class PlatformSignal(val sampledAt: Long, val allowed: Boolean)
        private val createdAt = SystemClock.elapsedRealtime()
        private val signalPending = AtomicBoolean(false)
        @Volatile private var signal: PlatformSignal? = null
        @Volatile private var signalTask: DebugBatchTask? = null
        fun platformReady(): Boolean = signal.let { DebugBatchGuard.platformReady(SystemClock.elapsedRealtime(), it?.sampledAt, it?.allowed) }
        fun bindingCurrent(): Boolean {
            val input = DebugAppliedInputs.witness.snapshot()
            val owner = MainKernelAttemptRegistry.ownerForDrain()?.attempt
            return input.stage == "NativeInputReturned" && input.startAcknowledged && input.revision == binding.revision &&
                input.runId == binding.runId && input.birthNonce == binding.birthNonce && input.configDigest == binding.digest &&
                owner?.runId == binding.runId && owner.birthNonce == binding.birthNonce && !owner.revoked
        }
        fun allowed() = armed && !lease.snapshot().sealed && SystemClock.elapsedRealtime() < deadline &&
                        SystemClock.elapsedRealtime() < controllerLeaseUntil && platformReady() && bindingCurrent()
        fun close(reason: String) {
            abortReason = abortReason ?: reason
            tcp.seal(); udp.seal()
            signalTask?.rejectBeforeRun()
            lease.seal()
            io.shutdownNow().forEach { (it as DebugBatchTask).rejectBeforeRun() }; guardian.shutdownNow()
            // No join here: a guardian or I/O callback may be the caller. External
            // cleanupObserve reads actual executor termination and the exact ledger.
        }
        fun complete(): Boolean {
            val s = lease.snapshot()
            return s.sealed && s.workers == 0 && s.acquiring == 0 && s.closing == 0 && s.handles == 0 &&
                   !s.closeFailed && io.isTerminated && guardian.isTerminated
        }
        fun submit(action: () -> Unit) {
            if (!lease.workerBorn()) return
            val task = DebugBatchTask(lease, action) { close("io-unavailable") }
            try { io.execute(task) }
            catch (_: Throwable) { task.rejectBeforeRun(); close("executor-rejected") }
        }
        fun guardTick(activity: Activity) {
            // The deadline owner never calls SDK/Binder or waits an observer. A
            // blocked/redacted SDK sample expires and aborts; its original lease
            // remains Unknown until that actual task returns.
            val now = SystemClock.elapsedRealtime()
            val previous = signal
            DebugBatchGuard.abortReason(now, createdAt, deadline, controllerLeaseUntil, bindingCurrent(), previous?.sampledAt, previous?.allowed)
                ?.let { close(it); return }
            if (previous != null && now - previous.sampledAt < 1000 || !signalPending.compareAndSet(false, true)) return
            if (!lease.workerBorn()) { signalPending.set(false); return }
            val task = DebugBatchTask(lease, {
                try {
                    if (lease.snapshot().sealed) return@DebugBatchTask
                    val power = activity.getSystemService(Context.POWER_SERVICE) as PowerManager
                    val keyguard = activity.getSystemService(Context.KEYGUARD_SERVICE) as KeyguardManager
                    val resumed = (activity as? LifecycleOwner)?.lifecycle?.currentState?.isAtLeast(Lifecycle.State.RESUMED) == true
                    val observed = PlatformSignal(SystemClock.elapsedRealtime(), power.isInteractive && !keyguard.isKeyguardLocked && resumed)
                    if (!lease.snapshot().sealed) signal = observed
                } finally { signalPending.set(false) }
            }) { close("guard-observer-unavailable") }
            signalTask = task
            try { DebugBatchCommandExecutor.value.execute(task) }
            catch (_: Throwable) { task.rejectBeforeRun(); close("guard-observer-rejected") }
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

    /** Caller input is intent only. Snapshot facts are sourced here, never supplied by JS. */
    fun command(activity: Activity, action: String, sessionId: String?, plan: String?): String {
        check(BuildConfig.DEBUG) { "Debug batch QA is disabled" } // before session/executor/socket creation
        require(action in setOf("prepare", "arm", "snapshot", "probe", "health", "close", "cleanupObserve"))
        if (action == "prepare") {
            val input = JSONObject(checkNotNull(plan))
            require(input.keys().asSequence().toSet() == setOf("peers", "port", "ttlMillis", "apkSha256", "expectedSourcePin", "sessionSecret", "runId", "birthNonce", "nativeInputRevision"))
            val peers = input.getJSONArray("peers").let { a -> (0 until a.length()).map(a::getString) }
            val port = input.getInt("port"); val ttl = input.getLong("ttlMillis")
            DebugBatchProtocol.validatePlan(peers, port, ttl)
            val sourcePin = input.getString("expectedSourcePin")
            require(sourcePin.matches(Regex("[0-9a-f]{64}")))
            val apkSha = activity.applicationInfo.sourceDir.let { filename ->
                val digest = MessageDigest.getInstance("SHA-256")
                java.io.File(filename).inputStream().use { stream -> val bytes = ByteArray(65536); while (true) { val n = stream.read(bytes); if (n < 0) break; digest.update(bytes, 0, n) } }
                DebugBatchProtocol.hex(digest.digest())
            }
            require(apkSha == input.getString("apkSha256")) { "actual APK differs" }
            val before = DebugAppliedInputs.witness.snapshot()
            val binding = Binding(input.getString("runId"), input.getString("birthNonce"), input.getLong("nativeInputRevision"), checkNotNull(before.configDigest))
            require(before.runId == binding.runId && before.birthNonce == binding.birthNonce && before.revision == binding.revision && before.stage == "NativeInputReturned" && before.startAcknowledged)
            val secret = DebugBatchProtocol.unhex(input.getString("sessionSecret"))
            val canonical = listOf(DebugBatchProtocol.SCHEMA, peers.sorted().joinToString(","), port.toString(), ttl.toString(), apkSha, sourcePin, binding.runId, binding.birthNonce, binding.revision.toString(), binding.digest).joinToString("|")
            val session = Session(DebugBatchProtocol.sha(canonical), apkSha, sourcePin, peers, port, SystemClock.elapsedRealtime() + ttl, secret, binding)
            val accepted = synchronized(gate) {
                if (active != null && !checkNotNull(active).complete()) false else {
                    active?.let { it.secret.fill(0); lastClosed = it }
                    active = session; true
                }
            }
            if (!accepted) { session.close("another-session"); secret.fill(0); error("another batch still owns resources") }
            try { session.guardian.scheduleAtFixedRate({ try { session.guardTick(activity) } catch (_: Throwable) { session.close("guardian-failed") } }, 0, 250, TimeUnit.MILLISECONDS) }
            catch (_: Throwable) { session.close("guardian-rejected") }
            return snapshot(activity, session)
        }
        val session = synchronized(gate) {
            active?.takeIf { it.sessionId == sessionId }
                ?: if (action == "cleanupObserve") lastClosed?.takeIf { it.sessionId == sessionId } else null
        } ?: error("session not owned by this app boot")
        when (action) {
            "arm" -> {
                check(session.bindingCurrent() && session.platformReady() && SystemClock.elapsedRealtime() < session.deadline)
                synchronized(session) { check(!session.armed && !session.lease.snapshot().sealed); session.armed = true }
                session.submit(session::tcpLoop); session.submit(session::udpLoop)
            }
            "health" -> if (!session.lease.snapshot().sealed) session.controllerLeaseUntil = minOf(session.deadline, SystemClock.elapsedRealtime() + 10000)
            "close" -> session.close("controller-close")
        }
        return snapshot(activity, session)
    }

    private fun snapshot(activity: Activity, session: Session): String {
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
            .put("source", "AppLive").put("bootNonce", bootNonce).put("sessionId", session.sessionId).put("nonce", session.nonce)
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
                .put("workers", lease.workers).put("acquiring", lease.acquiring).put("closing", lease.closing)
                .put("handles", lease.handles).put("closeFailed", lease.closeFailed).put("ioTerminated", session.io.isTerminated)
                .put("guardianTerminated", session.guardian.isTerminated).put("artifacts", "ArtifactsNotCreated")
                .put("independentOldSessionHealth", "NotObserved").put("mainNativeCleanup", "Unknown"))
            .put("cases", JSONObject().put("tcpWitness", outcome(session.tcp.snapshot())).put("udpAppSocket", outcome(session.udp.snapshot()))
                .put("witnessEvidenceScope", "AppSocketAuthenticatedRequestReceived").put("independentPeerAck", "NotObserved")
                .put("coreIngressAttribution", "Unknown").put("policyNegative", "NotObserved").put("tcpCoreOutbound", "NotObserved")
                .put("handover", "Deferred").put("dns", "Unknown").put("completeOsVpnScope", "Unknown"))
            .put("result", if (session.abortReason != null) "Aborted" else if (session.tcp.snapshot().matched + session.udp.snapshot().matched > 0) "ObservationsRecorded" else "PreparedOnly").toString()
    }
}
