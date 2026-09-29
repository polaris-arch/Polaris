package com.polaris2.app.vpn

import org.json.JSONException
import org.json.JSONObject

/** A buffered management RPC must never be delivered to a later native core at the same port. */
internal class DualModeEndpointTombstone {
    private val retired = mutableSetOf<Int>()

    class Retired : IllegalStateException("Android 双态内核的管理端点已退休；请回到 Polaris 重新连接")
    class ReloadRequiresReconnect : IllegalStateException("Android 双态内核需要通过 Polaris 重新连接以更换管理端点")
    class Invalid : IllegalArgumentException("Android 双态内核管理端点无效")

    /** Null means an ordinary config, whose native reload behavior remains unchanged. */
    fun endpoint(config: String): Int? {
        try {
            val root = JSONObject(config)
            val clash = root.optJSONObject("experimental")?.optJSONObject("clash_api") ?: return null
            val mode = clash.optString("default_mode", "")
            if (mode.isEmpty()) return null
            if (mode != "normal" && mode != "mesh-direct") throw Invalid()
            val services = root.optJSONArray("services") ?: throw Invalid()
            var port: Int? = null
            for (index in 0 until services.length()) {
                val service = services.getJSONObject(index)
                if (service.optString("type") != "api") continue
                if (port != null) throw Invalid()
                val value = service.opt("listen_port")
                if (value !is Number || value.toDouble() != value.toInt().toDouble() ||
                    value.toInt() !in 1..65535) throw Invalid()
                port = value.toInt()
            }
            return port ?: throw Invalid()
        } catch (_: JSONException) {
            // org.json exceptions may contain source JSON, including credentials.
            throw Invalid()
        }
    }

    /** Called before any native server creation; even a failed birth leaves its port retired. */
    @Synchronized
    fun claimBirth(config: String): Int? {
        val port = endpoint(config) ?: return null
        if (!retired.add(port)) throw Retired()
        return port
    }

    /** Bridge preflight only: rejection here proves no native attempt was dispatched. */
    @Synchronized
    fun requireFreshBridgeEndpoint(config: String) {
        val port = endpoint(config) ?: return
        if (port in retired) throw Retired()
    }

    fun requireReloadAllowed(currentDualMode: Boolean, nextConfig: String) {
        if (currentDualMode || endpoint(nextConfig) != null) throw ReloadRequiresReconnect()
    }
}

/** Process lifetime, including service destruction/recreation. Never clear on Stop or failure. */
internal val MainDualModeEndpointTombstone = DualModeEndpointTombstone()
