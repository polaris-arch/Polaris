// 本仓自有文件（非移植）。
package com.polaris2.app.vpn

/** 前台服务的生命周期状态。通知文案与「能否重复 start」都由它单点决定。 */
enum class ServiceState {
    Stopped,
    Starting,
    Started,
    Stopping,
}

/** Every rejected ownerless FGS request needs exact-startId teardown, including handoff invalidation. */
internal fun shouldStopUnownedServiceStart(
    state: ServiceState,
    hasAttempt: Boolean,
): Boolean = state == ServiceState.Stopped && !hasAttempt

/**
 * 进程内广播的 action 名。
 *
 * 为什么用广播而不是 Binder：通知栏按钮只能发 PendingIntent，而 PendingIntent 能落到的接收端里，
 * 广播是唯一不需要把 Service 暴露给外部的一种（`getService` 要求组件可被启动，`getActivity` 要多绕一屏）。
 * 全部带 `setPackage(自己)` 且接收方按 RECEIVER_NOT_EXPORTED 动态注册，不构成外部攻击面。
 */
object ServiceAction {
    const val SERVICE_STOP = "com.polaris2.app.vpn.SERVICE_STOP"
}
