package com.polaris2.app.vpn

/** Setup and loaded-file observations must happen in this order, also on cold start. */
internal object LoadedLibboxIdentity {
    fun <T> readAfterSetup(ready: () -> Boolean, setup: () -> Unit, completed: () -> Unit, read: () -> T): T {
        val wasReady = ready()
        setup()
        if (!wasReady) completed()
        return read()
    }

    fun matchesMapping(line: String, path: String, inode: Long, major: Long, minor: Long): Boolean {
        val fields = line.trim().split(Regex("\\s+"), limit = 6)
        val device = fields.getOrNull(3)?.split(':') ?: return false
        return fields.size == 6 && fields[5] == path && fields[4].toLongOrNull() == inode &&
            device.size == 2 && device[0].toLongOrNull(16) == major && device[1].toLongOrNull(16) == minor
    }
}
