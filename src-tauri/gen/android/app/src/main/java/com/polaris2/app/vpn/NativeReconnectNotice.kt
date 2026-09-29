package com.polaris2.app.vpn

import android.content.Context
import java.io.File

/** Process-scoped reason read by Rust's proxy status projection on app resume. */
internal object NativeReconnectNotice {
    // Same app_config_dir()/polaris as Rust ProxyRuntime::config.dir().
    private const val FILE_NAME = "native-reconnect-required"
    const val NOTIFICATION_ID = 39091
    private fun marker(context: Context) = File(File(context.dataDir, "polaris"), FILE_NAME)

    @Synchronized fun clear(context: Context) {
        clear(marker(context))
        runCatching { PolarisApplication.notification.cancel(NOTIFICATION_ID) }
    }

    @Synchronized internal fun clear(file: File) { file.delete() }

    @Synchronized fun require(context: Context, owner: String) = require(marker(context), owner)

    @Synchronized internal fun require(file: File, owner: String) {
        require(owner.isNotBlank())
        check(file.parentFile?.isDirectory == true || file.parentFile?.mkdirs() == true)
        file.writeText(owner)
    }

    @Synchronized fun owner(context: Context): String? = owner(marker(context))

    @Synchronized internal fun owner(file: File): String? =
        file.takeIf { it.isFile }?.readText()?.takeIf { it.isNotBlank() }

    /** A late close for A must not erase a newer B notice. */
    @Synchronized fun clearIfOwner(context: Context, expectedOwner: String): Boolean {
        val cleared = clearIfOwner(marker(context), expectedOwner)
        if (cleared) runCatching { PolarisApplication.notification.cancel(NOTIFICATION_ID) }
        return cleared
    }

    @Synchronized internal fun clearIfOwner(file: File, expectedOwner: String): Boolean =
        expectedOwner.isNotBlank() && owner(file) == expectedOwner && file.delete()
}
