package com.polaris2.app.vpn

import android.content.Context
import android.util.AtomicFile
import android.util.Log
import java.io.File

/** Materialize APK assets before Rust startup; Rust retains ownership of runtime rule seeding. */
internal object BundledRules {
    private const val ASSET_DIR = "_up_/resources/data"
    private const val EXTRACTED_DIR = "bundled-geo"

    fun prepare(context: Context) {
        val directory = File(context.dataDir, EXTRACTED_DIR)
        try {
            check(directory.isDirectory || directory.mkdirs()) { "Cannot create bundled geo directory" }
            val names = context.assets.list(ASSET_DIR).orEmpty().filter { it.endsWith(".srs") }
            check(names.isNotEmpty()) { "APK contains no bundled geo assets" }
            for (name in names) {
                val bytes = context.assets.open("$ASSET_DIR/$name").use { it.readBytes() }
                check(bytes.size > 3 && bytes[0] == 0x53.toByte() &&
                    bytes[1] == 0x52.toByte() && bytes[2] == 0x53.toByte()) { "Invalid SRS asset: $name" }
                val file = File(directory, name)
                if (runCatching { file.readBytes().contentEquals(bytes) }.getOrDefault(false)) continue
                val atomic = AtomicFile(file)
                val output = atomic.startWrite()
                try {
                    output.write(bytes)
                    atomic.finishWrite(output)
                } catch (error: Throwable) {
                    atomic.failWrite(output)
                    throw error
                }
            }
            Log.i("PolarisBundledRules", "Prepared ${names.size} APK rule assets in $directory")
        } catch (error: Exception) {
            // Keep recovery/export available; Rust reports the specific missing runtime tags.
            Log.e("PolarisBundledRules", "Cannot prepare bundled geo assets", error)
        }
    }
}
