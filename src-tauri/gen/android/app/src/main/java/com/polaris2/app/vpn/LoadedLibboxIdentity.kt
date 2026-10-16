package com.polaris2.app.vpn

/** Setup and loaded-file observations must happen in this order, also on cold start. */
internal object LoadedLibboxIdentity {
    fun <T> readAfterSetup(ready: () -> Boolean, setup: () -> Unit, completed: () -> Unit, read: () -> T): T {
        val wasReady = ready()
        setup()
        if (!wasReady) completed()
        return read()
    }

    // Android bionic sys/sysmacros.h (android-15.0.0_r1): dev_t uses
    // split 32-bit major/minor fields. Pure bit operations work on minSdk 24;
    // android.system.Os exposes no public Java major/minor methods.
    // https://android.googlesource.com/platform/bionic/+/refs/tags/android-15.0.0_r1/libc/include/sys/sysmacros.h
    fun deviceMajor(device: Long): Long = ((device ushr 32) and 0xfffff000L) or ((device ushr 8) and 0xfffL)
    fun deviceMinor(device: Long): Long = ((device ushr 12) and 0xffffff00L) or (device and 0xffL)

    fun matchesMapping(line: String, path: String, inode: Long, deviceNumber: Long): Boolean {
        val fields = line.trim().split(Regex("\\s+"), limit = 6)
        val device = fields.getOrNull(3)?.split(':') ?: return false
        return fields.size == 6 && fields[5] == path && fields[4].toLongOrNull() == inode &&
            device.size == 2 && device[0].toLongOrNull(16) == deviceMajor(deviceNumber) &&
            device[1].toLongOrNull(16) == deviceMinor(deviceNumber)
    }
}
