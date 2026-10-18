package com.polaris2.app.vpn

import org.junit.Assert.*
import org.junit.Test

class DefaultNetworkObservationTest {
    // Equality models Android Network identity; display/interface/cost never enter this ledger.
    private data class Net(val handle: Long)
    private val source = "12345678-1234-4234-9234-123456789abc"
    private fun known(ledger: DefaultNetworkObservation<Net>, session: Any, handle: Long) {
        ledger.available(session, Net(handle))
        ledger.capabilities(session, Net(handle), true)
    }
    @Test fun firstKnownAndRepeatedWrappersOnlyEstablishBaseline() {
        val ledger = DefaultNetworkObservation<Net>(source); val session = Any()
        ledger.start(session)
        assertFalse(ledger.snapshot().currentKnown)
        known(ledger, session, 7)
        repeat(4) { known(ledger, session, 7) }
        assertEquals("0", ledger.snapshot().seq)
        assertEquals(source, ledger.snapshot().sourceEpoch)
        assertTrue(ledger.snapshot().currentKnown)
    }
    @Test fun sameCostAndInterfaceNetworkChangesAndRoundTripAreRetainedBetweenPolls() {
        val ledger = DefaultNetworkObservation<Net>(source); val session = Any(); ledger.start(session)
        known(ledger, session, 7); known(ledger, session, 8); known(ledger, session, 7)
        assertEquals("2", ledger.snapshot().seq)
        assertTrue(ledger.snapshot().currentKnown)
    }
    @Test fun lateCapabilitiesAndLostForSupersededCandidateNeverChangeAnchor() {
        val ledger = DefaultNetworkObservation<Net>(source); val session = Any(); ledger.start(session)
        known(ledger, session, 7)
        ledger.available(session, Net(8)); ledger.available(session, Net(9))
        ledger.capabilities(session, Net(8), true); ledger.lost(session, Net(8))
        assertFalse(ledger.snapshot().currentKnown)
        assertEquals("0", ledger.snapshot().seq)
        ledger.capabilities(session, Net(9), true)
        assertEquals("1", ledger.snapshot().seq)
    }
    @Test fun vpnMissingProofAndNoAvailableCandidateAreUnavailable() {
        val ledger = DefaultNetworkObservation<Net>(source); val session = Any(); ledger.start(session)
        ledger.capabilities(session, Net(7), true) // no callback selection proof
        assertFalse(ledger.snapshot().currentKnown)
        ledger.available(session, Net(7)); ledger.capabilities(session, Net(7), false)
        assertFalse(ledger.snapshot().currentKnown)
        assertEquals("0", ledger.snapshot().seq)
        ledger.capabilities(session, Net(7), true)
        assertTrue(ledger.snapshot().currentKnown)
        ledger.available(session, Net(8)); ledger.capabilities(session, Net(8), false)
        known(ledger, session, 7)
        assertEquals("0", ledger.snapshot().seq)
        assertTrue(ledger.snapshot().coverageGap)
    }
    @Test fun gapThenSameNetworkPreservesButDifferentVerifiedNetworkGrows() {
        for (next in listOf(7L, 8L)) {
            val ledger = DefaultNetworkObservation<Net>(source); val session = Any(); ledger.start(session)
            known(ledger, session, 7); ledger.lost(session, Net(7))
            assertFalse(ledger.snapshot().currentKnown)
            known(ledger, session, next)
            assertEquals(if (next == 7L) "0" else "1", ledger.snapshot().seq)
            assertTrue(ledger.snapshot().coverageGap)
        }
    }
    @Test fun stopStartKeepsHistoryAndOldSessionsCannotPublishOrStopSuccessor() {
        val ledger = DefaultNetworkObservation<Net>(source); val old = Any(); ledger.start(old)
        known(ledger, old, 7); known(ledger, old, 8)
        ledger.stop(old)
        assertEquals("1", ledger.snapshot().seq)
        assertFalse(ledger.snapshot().currentKnown)
        val next = Any(); ledger.start(next)
        known(ledger, next, 7)
        known(ledger, old, 99); ledger.stop(old); ledger.lost(old, Net(7))
        assertEquals("2", ledger.snapshot().seq)
        assertEquals(source, ledger.snapshot().sourceEpoch)
        assertTrue(ledger.snapshot().currentKnown)
    }
    @Test fun unobservedStopGapRoundTripIsNeverInvented() {
        val ledger = DefaultNetworkObservation<Net>(source); val old = Any(); ledger.start(old)
        known(ledger, old, 7); ledger.stop(old)
        // Events while stopped have no admission. An unseen A->B->A cannot be claimed.
        known(ledger, old, 8); known(ledger, old, 7)
        val next = Any(); ledger.start(next); known(ledger, next, 7)
        assertEquals("0", ledger.snapshot().seq)
        assertTrue(ledger.snapshot().coverageGap)
    }
    @Test fun currentUnknownStillCarriesEveryPreviouslyConfirmedChange() {
        val ledger = DefaultNetworkObservation<Net>(source); val session = Any(); ledger.start(session)
        known(ledger, session, 7); known(ledger, session, 8); ledger.lost(session, Net(8))
        val snapshot = ledger.snapshot()
        assertEquals("1", snapshot.seq)
        assertFalse(snapshot.currentKnown)
        assertTrue(snapshot.coverageGap)
    }
    @Test fun exactLongCounterAndOverflowNeverRoundWrapOrRotateSource() {
        for (initial in listOf(9007199254740993L, Long.MAX_VALUE)) {
            val ledger = DefaultNetworkObservation<Net>(source); val session = Any(); ledger.start(session)
            known(ledger, session, 7)
            val field = ledger.javaClass.getDeclaredField("seq"); field.isAccessible = true; field.setLong(ledger, initial)
            known(ledger, session, 8)
            assertEquals(if (initial == Long.MAX_VALUE) initial.toString() else (initial + 1).toString(), ledger.snapshot().seq)
            assertEquals(source, ledger.snapshot().sourceEpoch)
            assertEquals(initial != Long.MAX_VALUE, ledger.snapshot().currentKnown)
        }
    }

}
