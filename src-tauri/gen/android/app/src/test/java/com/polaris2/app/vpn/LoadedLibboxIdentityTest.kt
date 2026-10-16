package com.polaris2.app.vpn

import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

class LoadedLibboxIdentityTest {
    @Test fun delayedSetupDoesNotPublishEarlyUnknownAndNotifiesRecovery() {
        val entered = CountDownLatch(1)
        val finish = CountDownLatch(1)
        val sampled = AtomicBoolean(false)
        val notified = AtomicBoolean(false)
        val worker = Thread {
            LoadedLibboxIdentity.readAfterSetup({ false }, {
                entered.countDown()
                check(finish.await(2, TimeUnit.SECONDS))
            }, { notified.set(true) }, {
                assertTrue(notified.get())
                sampled.set(true)
            })
        }
        worker.start()
        assertTrue(entered.await(2, TimeUnit.SECONDS))
        assertFalse(sampled.get())
        finish.countDown()
        worker.join(2000)
        assertFalse(worker.isAlive)
        assertTrue(sampled.get())
        LoadedLibboxIdentity.readAfterSetup({ true }, {}, { fail("ready queries must not trigger a refresh loop") }, {})
    }

    @Test fun failedSetupCannotReadOrClaimCompletion() {
        try {
            LoadedLibboxIdentity.readAfterSetup({ false }, { error("setup failed") }, { fail() }, { fail() })
            fail()
        } catch (expected: IllegalStateException) { assertEquals("setup failed", expected.message) }
    }

    @Test fun matchingPathWithoutActualDeviceAndInodeIsNotLoadedImageProof() {
        val path = "/data/app/lib/arm64/libbox.so"
        val line = "1000-2000 r-xp 00000000 08:02 42 $path"
        assertTrue(LoadedLibboxIdentity.matchesMapping(line, path, 42, 0x802L))
        assertFalse(LoadedLibboxIdentity.matchesMapping(line, path, 43, 0x802L))
        assertFalse(LoadedLibboxIdentity.matchesMapping(line, path, 42, 0x902L))
        assertFalse(LoadedLibboxIdentity.matchesMapping(line + " (deleted)", path, 42, 0x802L))
    }
    // Split high-bit fixture follows bionic makedev/major/minor, not a low-byte approximation.
    @Test fun splitHighDeviceBitsMatchProcMapsWithoutHiddenSdkMethods() {
        val device = 0x12345abcdefabc12L
        assertEquals(0x12345abcL, LoadedLibboxIdentity.deviceMajor(device))
        assertEquals(0xabcdef12L, LoadedLibboxIdentity.deviceMinor(device))
        assertEquals(0xffffffffL, LoadedLibboxIdentity.deviceMajor(-1L))
        assertEquals(0xffffffffL, LoadedLibboxIdentity.deviceMinor(-1L))
        val path = "/data/app/lib/arm64/libbox.so"
        val line = "1000-2000 r-xp 00000000 12345abc:abcdef12 42 $path"
        assertTrue(LoadedLibboxIdentity.matchesMapping(line, path, 42, device))
        assertFalse(LoadedLibboxIdentity.matchesMapping(line, path, 42, device xor 1L))
        assertFalse(LoadedLibboxIdentity.matchesMapping(line, path, 43, device))
    }

}
