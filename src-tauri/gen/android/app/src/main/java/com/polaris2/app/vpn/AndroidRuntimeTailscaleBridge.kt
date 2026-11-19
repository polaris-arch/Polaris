package com.polaris2.app.vpn

import io.nekohasekai.libbox.CommandServer
import io.nekohasekai.libbox.OverrideOptions
import io.nekohasekai.libbox.RuntimePersistenceHandler
import java.io.File
import java.io.IOException
import java.lang.reflect.InvocationTargetException
import java.lang.reflect.Method
import java.lang.reflect.Proxy
import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONObject

/** Typed Go V1 registration. An older selected libbox keeps its cold-only Tailscale path. */
internal object AndroidRuntimeTailscaleBridge {
    private const val CONTRACT = "polaris-android-runtime-ts-writer-v1"
    private const val ADMISSION_CONTRACT = "polaris-android-runtime-ts-admission-v1"
    private const val LIBBOX = "io.nekohasekai.libbox"
    private val hex64 = Regex("[0-9a-f]{64}")

    internal data class ControlledDirectories(val state: RuntimeTailscaleStatePaths, val taildrop: String) {
        val paths: Set<String> get() = setOf(state.canonical, taildrop)
    }

    /** An absent symbol alone selects the legacy call. A present method is never retried. */
    fun startMain(server: CommandServer, config: String, options: OverrideOptions, handler: RuntimePersistenceHandler): String? {
        val methods = server.javaClass.methods.filter { it.name == "startOrReloadServiceWithTailscaleStartup" }
        if (methods.isEmpty()) {
            server.startOrReloadServiceWithRetryableRuntimePersistence(config, options, handler)
            return null
        }
        val method = methods.single()
        check(method.returnType == String::class.java &&
            method.parameterTypes.contentEquals(arrayOf(String::class.java, OverrideOptions::class.java, RuntimePersistenceHandler::class.java)))
        return invoke(server, method, config, options, handler) as String
    }

    fun register(server: CommandServer, owner: MainKernelAttempt<CommandServer>, dataDir: File, current: () -> Boolean) {
        val setter = server.javaClass.methods.singleOrNull {
            it.name == "setRuntimeTailscalePlatform" && it.parameterTypes.size == 1
        } ?: return
        val platform = setter.parameterTypes.single()
        check(platform.isInterface && platform.name == "$LIBBOX.RuntimeTailscalePlatform")
        val claim = platform.methods.single { it.name == "claimRuntimeTailscale" }
        val scopeType = claim.parameterTypes.single()
        check(scopeType.name == "$LIBBOX.RuntimeTailscaleScope" && claim.returnType == Boolean::class.javaPrimitiveType)
        val release = platform.getMethod("releaseRuntimeTailscale", scopeType, Boolean::class.javaPrimitiveType)
        val validate = platform.getMethod("validateRuntimeTailscale", String::class.java, String::class.java, String::class.java)
        val version = platform.getMethod("runtimeTailscaleVersion")
        check(release.returnType == Void.TYPE && validate.returnType == Boolean::class.javaPrimitiveType &&
            version.returnType == Int::class.javaPrimitiveType)
        val readScope = scopeReader(scopeType)
        val ticket = checkNotNull(owner.nativeTicket)
        val ledger = AndroidNativeAdmissionGate.ledger
        val versionRead = AtomicBoolean()
        fun currentIncarnation(): String? = if (current() && owner.runtimeTailscaleRegistered(server) &&
            isCurrent(owner, ticket, ledger) && owner.tailscaleStore?.hasConfirmedCurrentStart() == true
        ) {
            owner.runtimeAdmission?.identity?.incarnation
        } else null

        val proxy = Proxy.newProxyInstance(platform.classLoader, arrayOf(platform)) { receiver, method, arguments ->
            val args = arguments.orEmpty()
            when (method) {
                version -> {
                    versionRead.set(true)
                    1
                }
                validate -> runCatching {
                    val tag = args[0] as String
                    val incarnation = currentIncarnation() ?: return@runCatching false
                    if (!validTag(tag)) return@runCatching false
                    val state = args[1] as String
                    val taildrop = args[2] as String
                    val directories = controlledDirectories(dataDir, state, taildrop) ?: return@runCatching false
                    TransientLoginHost.validateRuntimeDirectories(owner, directories.paths) {
                        currentIncarnation() == incarnation &&
                            ledger.canRetainRuntimeWriter(ticket, owner.birthNonce, incarnation, tag, state, taildrop, directories.state)
                    }
                }.getOrDefault(false)
                claim -> {
                    val scope = readScope(checkNotNull(args[0]))
                    val directories = if (validScope(scope)) controlledDirectories(dataDir, scope.stateDirectory, scope.taildropDirectory)
                        else null
                    directories != null && TransientLoginHost.claimRuntimeDirectories(owner, scope, directories.paths,
                        retainScope = { ledger.retainRuntimeWriter(ticket, owner.birthNonce, scope, directories.state) },
                        allowed = { currentIncarnation() == scope.incarnation })
                }
                release -> {
                    // Both certified no-acquisition and joined original Close release only their exact claim.
                    val scope = readScope(checkNotNull(args[0]))
                    val acquired = args[1] as Boolean
                    check(TransientLoginHost.releaseRuntimeDirectories(owner, scope)) {
                        "Android runtime Tailscale release did not match its original claim"
                    }
                    check(ledger.settleRuntimeWriter(ticket, owner.birthNonce, scope, acquired)) {
                        "Android runtime Tailscale release did not match its original history"
                    }
                    null
                }
                else -> when (method.name) {
                    "equals" -> receiver === args[0]
                    "hashCode" -> System.identityHashCode(receiver)
                    "toString" -> "PolarisRuntimeTailscaleV1"
                    else -> error("Android runtime Tailscale method is unknown")
                }
            }
        }
        owner.registerRuntimeTailscale(server) {
            invoke(server, setter, proxy)
            versionRead.get()
        }
    }

    fun checkAdmission(attempt: MainKernelAttempt<*>, ledger: AndroidNativeAdmission): JSONObject {
        val ticket = checkNotNull(attempt.nativeTicket)
        check(attempt.runtimeTailscaleRegistered() && attempt.ownershipState() == "acknowledged" &&
            attempt.runtimeAdmission?.identity != null && isCurrent(attempt, ticket, ledger) &&
            attempt.tailscaleStore?.hasConfirmedCurrentStart() == true)
        return JSONObject().put("contractVersion", ADMISSION_CONTRACT).put("processNonce", ledger.processNonce)
            .put("nativeTicketId", ticket.id).put("runId", attempt.runId).put("birthNonce", attempt.birthNonce)
    }

    fun joinsOriginalOnReload(
        attempt: MainKernelAttempt<CommandServer>, server: CommandServer, scope: RuntimeTailscaleWriterScope,
    ): Boolean {
        val ticket = attempt.nativeTicket ?: return false
        val ledger = AndroidNativeAdmissionGate.ledger
        return attempt.runtimeTailscaleRegistered(server) && isCurrent(attempt, ticket, ledger) &&
            attempt.runtimeAdmission?.identity?.incarnation == scope.incarnation &&
            ledger.runtimeWriterHistory(ticket, attempt.birthNonce, scope)?.outcome == AndroidNativeAdmission.RuntimeWriterOutcome.Held
    }

    /** Strict native Reload has returned; its old claims must already be settled before republishing. */
    fun requireReloadRetirement(attempt: MainKernelAttempt<CommandServer>) {
        val ticket = checkNotNull(attempt.nativeTicket)
        check(!TransientLoginHost.hasRuntimeDirectoryClaims(attempt) &&
            !AndroidNativeAdmissionGate.ledger.hasHeldRuntimeWriter(ticket, attempt.birthNonce)) {
            "Android runtime Tailscale reload retirement is unknown"
        }
    }

    fun observe(attempt: MainKernelAttempt<*>, digest: String): JSONObject {
        val ledger = AndroidNativeAdmissionGate.ledger
        val ticket = checkNotNull(attempt.nativeTicket)
        val identity = checkNotNull(attempt.runtimeAdmission?.identity)
        check(hex64.matches(digest))
        val scope = checkNotNull(TransientLoginHost.observeRuntimeScope(attempt, identity.incarnation, digest) { original ->
            isCurrent(attempt, ticket, ledger) && attempt.runtimeAdmission?.identity?.incarnation == identity.incarnation &&
                ledger.runtimeWriterHistory(ticket, attempt.birthNonce, original)?.outcome == AndroidNativeAdmission.RuntimeWriterOutcome.Held
        })
        return receipt(ledger.processNonce, ticket.id, attempt.runId, attempt.birthNonce, scope)
    }

    /** Reads only a retained terminal callback on the original Entry, including after registry release. */
    fun readHistory(binding: JSONObject, ledger: AndroidNativeAdmission): JSONObject {
        check(binding.keys().asSequence().toSet() == setOf("contractVersion", "processNonce", "nativeTicketId", "runId", "birthNonce", "scope"))
        check(binding.get("contractVersion") == CONTRACT && binding.get("processNonce") == ledger.processNonce)
        val ticketId = binding.get("nativeTicketId") as String
        val runId = binding.get("runId") as String
        val birthNonce = binding.get("birthNonce") as String
        val scope = decodeScope(binding.getJSONObject("scope"))
        val original = checkNotNull(ledger.terminalRuntimeWriterHistory(ticketId, runId, birthNonce, scope))
        return JSONObject().put("original", receipt(ledger.processNonce, ticketId, runId, birthNonce, original.scope))
            .put("outcome", original.outcome.name)
    }

    private fun receipt(processNonce: String, ticketId: String, runId: String, birthNonce: String, scope: RuntimeTailscaleWriterScope): JSONObject =
        JSONObject().put("contractVersion", CONTRACT).put("processNonce", processNonce).put("nativeTicketId", ticketId)
            .put("runId", runId).put("birthNonce", birthNonce).put("scope", scope.json())

    private fun isCurrent(attempt: MainKernelAttempt<*>, ticket: AndroidNativeAdmission.Ticket, ledger: AndroidNativeAdmission): Boolean =
        !attempt.revoked && attempt.nativeTicket === ticket && attempt.tailscaleStore?.ticket === ticket &&
            MainKernelAttemptRegistry.isCurrent(attempt) && ledger.state(ticket) == AndroidNativeAdmission.State.BirthEntered

    private fun validScope(scope: RuntimeTailscaleWriterScope): Boolean =
        scope.incarnation.isNotBlank() && scope.incarnation.length <= 256 && scope.resourceSequence != 0L &&
            scope.generation != 0L && hex64.matches(scope.runNonce) && hex64.matches(scope.configDigest) && validTag(scope.tag)

    private fun validTag(tag: String) = tag.isNotBlank() && tag.length <= 512 && tag.none(Char::isISOControl)

    private fun decodeScope(value: JSONObject): RuntimeTailscaleWriterScope {
        check(value.keys().asSequence().toSet() == setOf("incarnation", "resourceSequence", "generation",
            "runNonce", "configDigest", "tag", "stateDirectory", "taildropDirectory"))
        fun text(key: String) = value.get(key) as String
        fun unsigned(key: String): Long {
            val wire = text(key)
            val parsed = checkNotNull(wire.toULongOrNull())
            check(parsed.toString() == wire)
            return parsed.toLong()
        }
        return RuntimeTailscaleWriterScope(text("incarnation"), unsigned("resourceSequence"), unsigned("generation"),
            text("runNonce"), text("configDigest"), text("tag"), text("stateDirectory"), text("taildropDirectory"))
            .also { check(validScope(it)) }
    }

    internal fun controlledDirectories(dataDir: File, stateDirectory: String, taildropDirectory: String): ControlledDirectories? {
        if (stateDirectory.length > 4096 || taildropDirectory.length > 4096 ||
            '$' in stateDirectory || '$' in taildropDirectory) return null
        val state = File(stateDirectory)
        val taildrop = File(taildropDirectory)
        val id = state.name
        if (id.isEmpty() || id in setOf(".", "..") || id.toByteArray(Charsets.UTF_8).size > 255 ||
            id.any { it.isISOControl() || it == '\\' || it == ':' }) return null
        val relative = "polaris/tailscale/$id"
        return try {
            val nominal = File(dataDir, relative)
            val canonical = File(dataDir.canonicalFile, relative)
            val canonicalTaildrop = File(canonical, "Taildrop")
            if ((state != nominal && state != canonical) || taildrop != File(state, "Taildrop") ||
                state.canonicalFile != canonical || taildrop.canonicalFile != canonicalTaildrop) null
            else ControlledDirectories(RuntimeTailscaleStatePaths(canonical.path, nominal.path), canonicalTaildrop.path)
        } catch (_: IOException) {
            null
        }
    }

    /** Snapshot immutable Go getters into exact Kotlin value equality; never retain the generated proxy as a key. */
    private fun scopeReader(type: Class<*>): (Any) -> RuntimeTailscaleWriterScope {
        val incarnation = type.getMethod("incarnation")
        val sequence = type.getMethod("resourceSequence")
        val generation = type.getMethod("generation")
        val nonce = type.getMethod("runNonce")
        val digest = type.getMethod("configDigest")
        val tag = type.getMethod("tag")
        val state = type.getMethod("stateDirectory")
        val taildrop = type.getMethod("taildropDirectory")
        return { raw ->
            RuntimeTailscaleWriterScope(
                invoke(raw, incarnation) as String, invoke(raw, sequence) as Long, invoke(raw, generation) as Long,
                invoke(raw, nonce) as String, invoke(raw, digest) as String, invoke(raw, tag) as String,
                invoke(raw, state) as String, invoke(raw, taildrop) as String,
            )
        }
    }

    private fun invoke(receiver: Any, method: Method, vararg args: Any?): Any? = try {
        method.invoke(receiver, *args)
    } catch (failure: InvocationTargetException) {
        throw failure.targetException
    }
}
