package com.plugin.vpnservice

import org.junit.Test

import org.junit.Assert.assertArrayEquals

class TauriVpnServiceTest {
    @Test
    fun runtimePackageIsAlwaysExcluded() {
        assertArrayEquals(
            arrayOf("com.example.browser", "com.kkrainbow.easytier.policycandidate"),
            mergeDisallowedApplications(
                arrayOf("com.example.browser"),
                "com.kkrainbow.easytier.policycandidate",
            ),
        )
    }

    @Test
    fun runtimePackageIsNotDuplicated() {
        assertArrayEquals(
            arrayOf("com.kkrainbow.easytier"),
            mergeDisallowedApplications(
                arrayOf("com.kkrainbow.easytier"),
                "com.kkrainbow.easytier",
            ),
        )
    }
}
