package com.polaris2.app

import android.app.Activity
import android.content.Intent
import androidx.activity.result.ActivityResult
import androidx.activity.result.ActivityResultLauncher
import androidx.lifecycle.ViewModel
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import app.tauri.plugin.PluginManager
import java.util.UUID

@TauriPlugin
class BackupPickerPlugin(activity: Activity) : Plugin(activity) {
    @Command
    fun pickFile(invoke: Invoke) {
        val host = PluginManager.activity as? MainActivity
        if (host == null) {
            invoke.reject("The backup picker needs an active Polaris window")
            return
        }
        host.pickBackupFile(invoke.getArgs().getString("defaultName", null), invoke)
    }
}

// The registry key and pending invocation survive configuration changes together.
// A different owner gets a fresh key so it cannot receive an abandoned result.
internal class BackupFilePicker : ViewModel() {
    private data class Pending(val invoke: Invoke, val save: Boolean)

    val registryKey = "polaris.backup-file-picker.${UUID.randomUUID()}"
    private var pending: Pending? = null

    fun launch(launcher: ActivityResultLauncher<Intent>, defaultName: String?, invoke: Invoke) {
        if (pending != null) {
            invoke.reject("The backup picker is already open")
            return
        }
        val save = defaultName != null
        val action = if (save) Intent.ACTION_CREATE_DOCUMENT else Intent.ACTION_GET_CONTENT
        val intent = Intent(action).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*")
        if (defaultName != null) intent.putExtra(Intent.EXTRA_TITLE, defaultName)
        pending = Pending(invoke, save)
        try {
            launcher.launch(intent)
        } catch (_: Exception) {
            pending = null
            invoke.reject("Could not open the backup picker")
        }
    }

    fun complete(result: ActivityResult) {
        val request = pending ?: return
        pending = null
        val resultData = result.data.takeIf { result.resultCode == Activity.RESULT_OK }
        val clipUri = if (request.save) null else resultData?.clipData
            ?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.uri
        val uri = clipUri ?: resultData?.data
        request.invoke.resolve(JSObject().put("file", uri?.toString()))
    }

    override fun onCleared() {
        val request = pending
        pending = null
        request?.invoke?.reject("The backup picker window closed")
    }
}
