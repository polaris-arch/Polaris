package com.polaris2.app.vpn

import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

/** Only initialized after the plugin's release guard; bounded metadata command queue. */
internal object DebugBatchCommandExecutor {
    val value by lazy {
        ThreadPoolExecutor(0, 1, 10, TimeUnit.SECONDS, ArrayBlockingQueue(8),
            { Thread(it, "polaris-qa-command") }, ThreadPoolExecutor.AbortPolicy())
    }
}
