package com.polaris2.app.vpn

import org.json.JSONObject

/** Protocol evidence from the already source-verified libbox; this is not a source signature. */
internal data class RuntimeProtocolAdmission(
    val manifestDigest: String,
    val identity: Identity?,
    val controlRevision: String,
    val unsupportedPaths: List<String>,
) {
    data class Identity(val incarnation: String, val appliedRevision: String, val effectiveDigest: String)

    companion object {
        private val methods = listOf("GetRuntimeCapabilities", "PlanRuntimeConfig", "ApplyRuntimeConfig",
            "GetRuntimeConfigStatus", "RetryRuntimePersistence", "AcknowledgeRuntimePersistence")
        private val hexDigest = Regex("[0-9a-f]{64}")
        private val decimal = Regex("0|[1-9][0-9]{0,19}")
        private const val MAX_UINT64 = "18446744073709551615"

        /** Only protojson decimal strings are accepted for uint64; never pass through Double/Long. */
        private fun uint64(objectValue: JSONObject, key: String): String {
            val value = if (!objectValue.has(key)) "0" else objectValue.get(key) as? String
            require(value != null && decimal.matches(value) &&
                (value.length < MAX_UINT64.length || value <= MAX_UINT64)) { "invalid runtime integer" }
            return value
        }

        private fun text(objectValue: JSONObject, key: String): String =
            (if (objectValue.has(key)) objectValue.get(key) as? String else "")
                ?: throw IllegalArgumentException("invalid runtime text")

        private fun boolean(objectValue: JSONObject, key: String): Boolean =
            (if (objectValue.has(key)) objectValue.get(key) as? Boolean else false)
                ?: throw IllegalArgumentException("invalid runtime flag")

        private fun conditions(field: JSONObject): List<String> {
            val values = if (!field.has("conditions")) return emptyList() else field.getJSONArray("conditions")
            return (0 until values.length()).map {
                values.get(it) as? String ?: throw IllegalArgumentException("invalid runtime condition")
            }
        }

        /** BuildConfig.FLAVOR is the selected APK ABI, not a core-supplied platform claim. */
        fun androidPlatform(flavor: String, is64Bit: Boolean, processAbis: List<String>): String {
            val arch = when (flavor) {
                "arm64" -> "arm64"
                "arm" -> "arm"
                "x86" -> "386"
                "x86_64" -> "amd64"
                "universal" -> when (processAbis.firstOrNull()) {
                    "arm64-v8a" -> "arm64"
                    "armeabi-v7a" -> "arm"
                    "x86" -> "386"
                    "x86_64" -> "amd64"
                    else -> throw IllegalArgumentException("unknown Android process ABI")
                }
                else -> throw IllegalArgumentException("unknown Android build ABI")
            }
            require(is64Bit == (arch == "arm64" || arch == "amd64")) { "Android process ABI differs from build" }
            return "android/$arch"
        }

        fun decode(document: String, platform: String): RuntimeProtocolAdmission {
            require(document.toByteArray(Charsets.UTF_8).size in 1..(1024 * 1024)) { "runtime capabilities exceed budget" }
            val root = JSONObject(document)
            val contract = root.get("contract_version")
            require((contract is Int || contract is Long) && (contract as Number).toLong() == 1L) { "runtime contract mismatch" }
            val manifest = text(root, "implementation_manifest_digest")
            require(hexDigest.matches(manifest)) { "runtime manifest missing or invalid" }
            val fields = root.getJSONArray("fields")
            val entries = (0 until fields.length()).map { fields.getJSONObject(it) }
            val protocol = entries.filter { text(it, "effective_path") == "runtime.protocol" }.singleOrNull()
                ?: throw IllegalArgumentException("runtime protocol missing or duplicated")
            require(text(protocol, "variant") == "polaris-daily-runtime-v1" &&
                text(protocol, "adapter_version") == "1" && text(protocol, "platform_build") == platform &&
                conditions(protocol) == methods && (!protocol.has("update_class") ||
                    protocol.get("update_class") == "RUNTIME_UPDATE_UNSPECIFIED")) { "runtime protocol mismatch" }
            val reasons = entries.filter { text(it, "effective_path") == "runtime.instance" }
            val unsupported = mutableListOf<String>()
            var startedUnsupported = false
            for (reason in reasons) {
                val paths = conditions(reason)
                when (text(reason, "variant")) {
                    "OWNER_PERSISTENCE_UNBOUND" -> require(paths == listOf("runtime.persistence")) { "invalid persistence warning" }
                    "RUNTIME_STARTED_UNSUPPORTED" -> {
                        require(!startedUnsupported && paths == listOf("runtime")) { "invalid unsupported lifecycle" }
                        startedUnsupported = true
                    }
                    "PENDING_ADAPTER" -> {
                        require(paths.size == 1 && paths[0].isNotBlank() && paths[0] != "runtime") { "missing unsupported graph path" }
                        unsupported += paths[0]
                    }
                    // Starting/stopped/unavailable/unknown states are never native ready.
                    else -> throw IllegalArgumentException("runtime instance is not ready")
                }
            }
            val active = if (!root.has("active")) JSONObject() else root.getJSONObject("active")
            val incarnation = text(active, "incarnation")
            val revision = uint64(active, "applied_revision")
            val effective = text(active, "effective_digest")
            val enabled = boolean(root, "connected_hot_enabled")
            val identity = if (enabled) {
                require(!startedUnsupported && unsupported.isEmpty() && incarnation.isNotBlank() &&
                    incarnation.length <= 256 && revision != "0" && hexDigest.matches(effective)) { "runtime ready identity is invalid" }
                Identity(incarnation, revision, effective)
            } else {
                require(startedUnsupported && unsupported.isNotEmpty() && incarnation.isEmpty() &&
                    revision == "0" && effective.isEmpty()) { "runtime readiness is unproved" }
                null
            }
            // Omitted false/default fields are valid protojson. Full-schema and build-tag metadata
            // are not the connected graph's readiness gate. The observed manifest is not a pin.
            return RuntimeProtocolAdmission(manifest, identity, uint64(root, "control_revision"), unsupported)
        }
    }
}
