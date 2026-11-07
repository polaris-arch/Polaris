package com.polaris2.app.vpn

import android.os.ParcelFileDescriptor
import io.nekohasekai.libbox.CommandServer
import io.nekohasekai.libbox.TunOptions
import java.io.Closeable
import java.lang.reflect.InvocationHandler
import java.lang.reflect.InvocationTargetException
import java.lang.reflect.Method
import java.lang.reflect.Proxy

/** Fully configured Builder is private and never mutated after capture; establish replays its inputs. */
internal class AndroidTunSettings(val scope: TunScope, private val open: () -> ParcelFileDescriptor) {
    fun establish(): ParcelFileDescriptor = open()
}

/** Optional generated V1 binding. No reference to absent AAR types reaches Kotlin compilation. */
internal class AndroidNativeTunBridge(
    val attempt: MainKernelAttempt<CommandServer>,
    private val capture: (TunOptions) -> AndroidTunSettings,
    private val current: () -> Unit,
    stillOwner: () -> Unit,
    onUnknown: (Throwable) -> Unit,
) : Closeable {
    private val owner = NativeTunOwner<AndroidTunSettings, ParcelFileDescriptor>(
        current, stillOwner, AndroidTunSettings::establish, { it.fd }, { it.scope }, onUnknown,
    )
    private var platformProxy: Any? = null

    fun installLegacy(settings: AndroidTunSettings, descriptor: ParcelFileDescriptor): Int = owner.installLegacy(settings, descriptor)
    fun currentScope(): TunScope? = owner.currentScope()
    override fun close() = owner.close()

    fun register(server: CommandServer): Boolean {
        current()
        val setter = server.javaClass.methods.singleOrNull {
            it.name == "setRuntimeTunPlatform" && it.parameterTypes.size == 1
        } ?: return false // Old selected AAR: ordinary cold OpenTun, no native hot permission.
        check(platformProxy == null) { "android: native TUN bridge already registered" }
        val platformType = setter.parameterTypes.single()
        check(platformType.isInterface && platformType.name == "io.nekohasekai.libbox.RuntimeTunPlatform")
        val create = platformType.methods.single { it.name == "newTunSession" }
        check(create.parameterTypes.size == 3 && create.parameterTypes[0] == TunOptions::class.java)
        val sessionType = create.returnType
        check(sessionType.isInterface && sessionType.name == "io.nekohasekai.libbox.RuntimeTunSession" &&
            create.parameterTypes[1] == sessionType && create.parameterTypes[2].name == "io.nekohasekai.libbox.RuntimeTunReceipt")
        val proxy = Proxy.newProxyInstance(platformType.classLoader, arrayOf(platformType)) { receiver, method, arguments ->
            objectMethod(receiver, method, arguments) ?: when (method.name) {
                "runtimeTunVersion" -> 1
                "newTunSession" -> {
                    val args = checkNotNull(arguments)
                    val previous = args[1]?.let { value ->
                        check(Proxy.isProxyClass(value.javaClass)) { "android: native TUN previous session is foreign" }
                        val handler = Proxy.getInvocationHandler(value) as? SessionHandler
                            ?: error("android: native TUN previous session is foreign")
                        check(handler.bridge === this && handler.proxy === value) { "android: native TUN previous attempt differs" }
                        handler.session
                    }
                    val rawReceipt = checkNotNull(args[2])
                    val receipt = object : NativeTunReceipt {
                        override val ownerId = invoke(rawReceipt, rawReceipt.javaClass.getMethod("ownerID")) as String
                        override fun published() = invoke(rawReceipt, rawReceipt.javaClass.getMethod("published")) as Boolean
                    }
                    val session = owner.newSession(capture(args[0] as TunOptions), previous, receipt)
                    val handler = SessionHandler(this, session)
                    Proxy.newProxyInstance(sessionType.classLoader, arrayOf(sessionType), handler).also { handler.proxy = it }
                }
                else -> error("android: unknown native TUN platform method")
            }
        }
        current()
        invoke(server, setter, proxy)
        platformProxy = proxy // Keep the original callback object alive until exact native close.
        return true
    }

    private class SessionHandler(val bridge: AndroidNativeTunBridge, val session: NativeTunSession) : InvocationHandler {
        var proxy: Any? = null
        override fun invoke(receiver: Any, method: Method, args: Array<out Any?>?): Any? =
            objectMethod(receiver, method, args) ?: when (method.name) {
                "ownerID" -> session.ownerId
                "prepare" -> { session.prepare(); null }
                "fileDescriptor" -> session.fileDescriptor()
                "validate" -> { session.validate(); null }
                "abort" -> { session.abort(); null }
                "retire" -> { session.retire(); null }
                else -> error("android: unknown native TUN session method")
            }
    }

    companion object {
        private fun objectMethod(receiver: Any, method: Method, args: Array<out Any?>?): Any? = when (method.name) {
            "equals" -> receiver === args?.get(0)
            "hashCode" -> System.identityHashCode(receiver)
            "toString" -> "PolarisNativeTunV1"
            else -> null
        }
        private fun invoke(receiver: Any, method: Method, vararg args: Any?): Any? = try { method.invoke(receiver, *args) }
        catch (failure: InvocationTargetException) { throw failure.targetException }
    }
}
