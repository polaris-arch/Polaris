package com.polaris2.app.vpn

import android.app.ActivityManager
import android.content.Context
import android.os.Build
import android.os.Process
import android.util.Log
import androidx.webkit.WebViewCompat
import com.polaris2.app.BuildConfig
import java.io.File
import java.io.RandomAccessFile
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.system.exitProcess

/** Debug-only local evidence. Exported text must pass the Rust report redactor before sharing. */
object DebugDiagnostics {
    private const val LIMIT = 128 * 1024
    private val installed = AtomicBoolean(false)

    fun install(context: Context) {
        if (!BuildConfig.DEBUG || !installed.compareAndSet(false, true)) return
        val file = File(context.noBackupFilesDir, "debug-last-crash.txt")
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            try {
                runCatching {
                    file.writeText("time=${System.currentTimeMillis()} thread=${thread.name}\n" +
                        Log.getStackTraceString(error).take(LIMIT))
                }
            } finally {
                // Keep Android's normal crash handling; recording must never swallow a fatal error.
                if (previous != null) previous.uncaughtException(thread, error)
                else {
                    Process.killProcess(Process.myPid())
                    exitProcess(10)
                }
            }
        }
    }

    private fun tail(file: File): String {
        if (!file.isFile) return "(unavailable)"
        return RandomAccessFile(file, "r").use { input ->
            val size = minOf(input.length(), LIMIT.toLong()).toInt()
            input.seek(input.length() - size)
            val bytes = ByteArray(size)
            input.readFully(bytes)
            bytes.toString(Charsets.UTF_8)
        }
    }

    /** Runs on a worker: logcat is bounded by line count, time, and bytes read into IPC. */
    @Synchronized
    fun collect(context: Context): String {
        check(BuildConfig.DEBUG)
        return buildString {
            appendLine("Android ${Build.VERSION.RELEASE} / SDK ${Build.VERSION.SDK_INT}")
            appendLine("Device: ${Build.MANUFACTURER} ${Build.MODEL}; ABI: ${Build.SUPPORTED_ABIS.joinToString()}")
            appendLine("App: ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE}) debug")
            appendLine("WebView: ${WebViewCompat.getCurrentWebViewPackage(context)?.versionName ?: "unknown"}")
            appendLine("\nSocket binding probe (framework layer; no remote traffic):")
            appendLine(runCatching { "NIC_BIND_PROBE=" + InterfaceBindingDiagnostics.collect() }
                .getOrDefault("NIC_BIND_PROBE_UNAVAILABLE"))
            appendLine("\nRecent process exits (timestamps are epoch milliseconds):")
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                runCatching {
                    val manager = context.getSystemService(ActivityManager::class.java)
                    val exits = manager.getHistoricalProcessExitReasons(context.packageName, 0, 5)
                    if (exits.isEmpty()) appendLine("(no retained exit records)")
                    for (exit in exits) {
                        appendLine("time=${exit.timestamp} process=${exit.processName} pid=${exit.pid} " +
                            "reason=${exit.reason} status=${exit.status} importance=${exit.importance} " +
                            "pssKB=${exit.pss} rssKB=${exit.rss} description=${exit.description}")
                    }
                    appendLine("Reasons: 2=signal, 3=low memory, 4=Java crash, 5=native crash, 6=ANR, 10=user requested.")
                }.onFailure { appendLine("Exit records unavailable: ${it.javaClass.simpleName}") }
            } else appendLine("(requires Android 11+)")

            appendLine("\nLast recorded Java exception:")
            appendLine(runCatching { tail(File(context.noBackupFilesDir, "debug-last-crash.txt")) }
                .getOrDefault("(unavailable)"))
            appendLine("\nRecent app-UID logcat (main/system/crash, maximum 600 entries):")
            val output = File(context.cacheDir, "debug-logcat.tmp")
            var process: java.lang.Process? = null
            try {
                process = ProcessBuilder("logcat", "-d", "-t", "600", "-v", "threadtime",
                    "-b", "main", "-b", "system", "-b", "crash", "--uid=${Process.myUid()}")
                    .redirectErrorStream(true).redirectOutput(output).start()
                if (!process.waitFor(3, TimeUnit.SECONDS)) {
                    process.destroyForcibly()
                    appendLine("(logcat timed out; partial output follows)")
                } else if (process.exitValue() != 0) appendLine("(logcat unavailable on this device)")
                appendLine(tail(output))
            } catch (error: Exception) {
                appendLine("Logcat unavailable: ${error.javaClass.simpleName}")
            } finally {
                process?.destroy()
                output.delete()
            }
            appendLine("Native tombstone binary is not included; use ADB for a complete native backtrace.")
        }
    }
}
