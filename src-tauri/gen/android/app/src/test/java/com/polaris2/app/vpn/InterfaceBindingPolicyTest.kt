package com.polaris2.app.vpn

import java.io.Closeable
import org.junit.Assert.*
import org.junit.Test

class InterfaceBindingPolicyTest {
    private fun row(network: String, name: String = "wlan0", physical: Boolean = true) =
        InterfaceBindingCandidate(network, name, physical)

    private fun failure(code: String, action: () -> Unit) {
        try { action(); fail("expected $code") }
        catch (error: InterfaceBindingFailure) { assertEquals(code, error.code) }
    }

    @Test fun choosesNamedPhysicalNetworkInsteadOfDefault() {
        assertEquals("cellular", requireInterfaceNetwork("rmnet0", listOf(row("wifi"), row("cellular", "rmnet0"))))
    }

    @Test fun refusesAbsentVpnAndAmbiguousMatches() {
        failure("BIND_INTERFACE_UNAVAILABLE") { requireInterfaceNetwork("missing", listOf(row("wifi"))) }
        failure("BIND_INTERFACE_UNAVAILABLE") { requireInterfaceNetwork("tun0", listOf(row("vpn", "tun0", false))) }
        failure("BIND_INTERFACE_AMBIGUOUS") { requireInterfaceNetwork("wlan0", listOf(row("a"), row("b"))) }
        failure("BIND_INTERFACE_AMBIGUOUS") { requireInterfaceNetwork("wlan0", listOf(row("same"), row("same"))) }
        failure("BIND_INTERFACE_INVALID") { requireInterfaceNetwork(" wlan0", listOf(row("wifi"))) }
    }

    @Test fun restrictedImsIsRejectedButUnrestrictedLanCanBind() {
        assertFalse(isBindablePhysicalNetwork(true, false, false))
        assertFalse(isBindablePhysicalNetwork(false, true, true))
        // No INTERNET capability is part of this policy; unrestricted physical LAN remains usable.
        assertTrue(isBindablePhysicalNetwork(true, false, true))
        val restricted = row("ims", "rmnet0", isBindablePhysicalNetwork(true, false, false))
        failure("BIND_INTERFACE_UNAVAILABLE") {
            bindInterfaceSocket<String, Borrowed>(17, "rmnet0", { listOf(restricted) },
                { fail("restricted network must not borrow fd"); Borrowed() },
                { fail("protect") }, { _, _ -> fail("bind") })
        }
        val lan = row("lan", "eth0", isBindablePhysicalNetwork(true, false, true))
        assertEquals("lan", requireInterfaceNetwork("eth0", listOf(restricted, lan)))
    }

    private class Borrowed : Closeable { var closed = false; override fun close() { closed = true } }

    @Test fun protectsOriginalBindsDuplicateThenClosesOnlyDuplicate() {
        val duplicate = Borrowed(); val actions = mutableListOf<String>()
        bindInterfaceSocket(17, "wlan0", { listOf(row("wifi")) },
            { assertEquals(17, it); actions += "dup"; duplicate },
            { assertEquals(17, it); actions += "protect" },
            { network, fd -> assertEquals("wifi", network); assertSame(duplicate, fd); actions += "bind" })
        assertEquals(listOf("dup", "protect", "bind"), actions)
        assertTrue(duplicate.closed)
    }

    @Test fun disappearingNetworkClosesDuplicateWithoutProtectOrBind() {
        val duplicate = Borrowed(); var reads = 0
        failure("BIND_INTERFACE_UNAVAILABLE") {
            bindInterfaceSocket(17, "wlan0", { if (reads++ == 0) listOf(row("wifi")) else emptyList() },
                { duplicate }, { fail("protect") }, { _, _ -> fail("bind") })
        }
        assertTrue(duplicate.closed)
    }

    @Test fun replacementNetworkIsNotSilentlyChosen() {
        val duplicate = Borrowed(); var reads = 0
        failure("BIND_INTERFACE_NETWORK_CHANGED") {
            bindInterfaceSocket(17, "wlan0", { listOf(row(if (reads++ == 0) "old" else "new")) },
                { duplicate }, { fail("protect") }, { _, _ -> fail("bind") })
        }
        assertTrue(duplicate.closed)
    }

    @Test fun protectOrBindFailureClosesDuplicateAndPropagates() {
        for (stage in listOf("protect", "bind")) {
            val duplicate = Borrowed()
            var bindCalled = false
            try {
                bindInterfaceSocket(17, "wlan0", { listOf(row("wifi")) }, { duplicate },
                    { if (stage == "protect") throw IllegalStateException(stage) },
                    { _, _ -> bindCalled = true; throw IllegalStateException(stage) })
                fail("failure swallowed")
            } catch (error: IllegalStateException) { assertEquals(stage, error.message) }
            assertTrue(duplicate.closed)
            assertEquals(stage == "bind", bindCalled)
        }
    }

    @Test fun invalidFdFailsBeforeAnyBorrowOrNetworkLookup() {
        failure("BIND_INTERFACE_BAD_FD") {
            bindInterfaceSocket<String, Borrowed>(-1, "wlan0", { fail("lookup"); emptyList() },
                { fail("borrow"); Borrowed() }, { fail("protect") }, { _, _ -> fail("bind") })
        }
    }
}
