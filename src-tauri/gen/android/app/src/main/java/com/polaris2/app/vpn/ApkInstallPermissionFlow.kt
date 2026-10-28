package com.polaris2.app.vpn

/** One foreground update click. Permission callbacks never own an APK or install action. */
internal class ApkInstallPermissionFlow<T : Any> {
    data class Pending<T>(val invoke: T, val deadline: Long, var leftActivity: Boolean = false)

    private var pending: Pending<T>? = null
    private var installerBusy = false
    private var installerLeftActivity = false
    private var destroyed = false

    @Synchronized fun beginPermission(invoke: T, deadline: Long): Boolean {
        if (destroyed || pending != null || installerBusy) return false
        pending = Pending(invoke, deadline)
        return true
    }

    @Synchronized fun permissionPending(): Boolean = pending != null
    @Synchronized fun busy(): Boolean = pending != null || installerBusy

    @Synchronized fun paused() {
        pending?.leftActivity = true
        if (installerBusy) installerLeftActivity = true
    }

    @Synchronized fun resumed() {
        if (installerLeftActivity) {
            installerBusy = false
            installerLeftActivity = false
        }
    }

    /** Identity-bound result, or a resume after actually leaving for settings; consume once. */
    @Synchronized fun takeReturned(invoke: T? = null): Pending<T>? {
        val current = pending ?: return null
        if (invoke != null && current.invoke !== invoke) return null
        if (invoke == null && !current.leftActivity) return null
        pending = null
        return current
    }

    @Synchronized fun expire(invoke: T, now: Long): T? {
        val current = pending ?: return null
        if (current.invoke !== invoke || now < current.deadline) return null
        pending = null
        return current.invoke
    }

    @Synchronized fun beginInstall(): Boolean {
        if (destroyed || pending != null || installerBusy) return false
        installerBusy = true
        return true
    }

    @Synchronized fun finishInstall(handedOff: Boolean) {
        if (!handedOff) {
            installerBusy = false
            installerLeftActivity = false
        }
    }

    @Synchronized fun destroy(): T? {
        destroyed = true
        val invoke = pending?.invoke
        pending = null
        installerBusy = false
        return invoke
    }
}
