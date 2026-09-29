package com.polaris2.app.vpn

import android.content.Context
import java.io.File

/** Process-scoped reason read by Rust's proxy status projection on app resume. */
internal object NativeReconnectNotice {
    // Same app_config_dir()/polaris as Rust ProxyRuntime::config.dir().
    private const val FILE_NAME = "native-reconnect-required"
    private fun marker(context: Context) = File(File(context.dataDir, "polaris"), FILE_NAME)

    fun clear(context: Context) {
        marker(context).delete()
    }

    fun require(context: Context) {
        val file = marker(context)
        check(file.parentFile?.isDirectory == true || file.parentFile?.mkdirs() == true)
        file.writeText("dual-mode-reload-requires-reconnect")
    }
}
