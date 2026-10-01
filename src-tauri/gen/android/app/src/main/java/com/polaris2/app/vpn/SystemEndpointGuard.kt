package com.polaris2.app.vpn

import org.json.JSONObject
import org.json.JSONException

/** Final libbox config guard. Android's single VpnService TUN cannot own endpoint system NICs. */
internal object SystemEndpointGuard {
    const val ERROR = "SYSTEM_INTERFACE_UNSUPPORTED"
    private const val INVALID = "Invalid libbox endpoint configuration"

    class Unsupported : IllegalArgumentException(ERROR)

    private fun requestsSystemField(value: Any?): Boolean = when (value) {
        null, JSONObject.NULL, false -> false
        true -> true
        else -> throw JSONException(INVALID)
    }

    fun requireSupported(config: String) {
        try {
            val root = JSONObject(config)
            val value = root.opt("endpoints")
            if (value == null || value == JSONObject.NULL) return
            val endpoints = root.getJSONArray("endpoints")
            for (index in 0 until endpoints.length()) {
                val endpoint = endpoints.getJSONObject(index)
                if (requestsSystemField(endpoint.opt("system")) ||
                    requestsSystemField(endpoint.opt("system_interface"))) {
                    throw Unsupported()
                }
            }
        } catch (_: JSONException) {
            // Android's org.json exception text can include source input; never echo credentials.
            throw IllegalArgumentException(INVALID)
        }
    }
}
