package com.polaris2.app.vpn

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Android's real org.json parser must reject unsafe final configs before native setup. */
class TransientSpeedtestConfigGuardTest {
    private val secret = "0123456789abcdef0123456789abcdef"

    private fun config(): JSONObject {
        fun inbound(tag: String, port: Int) = JSONObject()
            .put("type", "http")
            .put("tag", tag)
            .put("listen", "127.0.0.1")
            .put("listen_port", port)
            .put("users", JSONArray().put(JSONObject().put("username", "polaris-temp").put("password", secret)))
        return JSONObject()
            .put("inbounds", JSONArray().put(inbound("one", 10101)).put(inbound("two", 10102)))
            .put("endpoints", JSONArray().put(JSONObject().put("type", "wireguard")))
            .put("outbounds", JSONArray().put(JSONObject().put("type", "direct")))
    }

    private fun rejected(config: JSONObject) {
        val error = runCatching { TransientSpeedtestHost.validateConfig(config.toString()) }.exceptionOrNull()
        assertTrue(error is IllegalArgumentException)
        assertEquals("Android 临时测速配置被拒绝", error?.message)
    }

    @Test fun allowsAuthenticatedLoopbackHttpWithPreservedEndpoint() {
        TransientSpeedtestHost.validateConfig(config().toString())
    }

    @Test fun rejectsTailscaleInEndpointAndRawOutbound() {
        rejected(config().apply { getJSONArray("endpoints").getJSONObject(0).put("type", "tailscale") })
        rejected(config().apply { getJSONArray("outbounds").getJSONObject(0).put("type", "tailscale") })
    }

    @Test fun rejectsNonLoopbackTunSharedCacheAndMissingAuth() {
        rejected(config().apply { getJSONArray("inbounds").getJSONObject(0).put("listen", "0.0.0.0") })
        rejected(config().apply { getJSONArray("inbounds").getJSONObject(0).put("type", "tun") })
        rejected(config().apply { put("experimental", JSONObject().put("cache_file", JSONObject())) })
        rejected(config().apply { getJSONArray("inbounds").getJSONObject(0).remove("users") })
    }
}
