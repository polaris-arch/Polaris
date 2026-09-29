// 本仓自有文件（非移植）。
package com.polaris2.app

import android.app.backup.BackupAgent
import android.app.backup.BackupDataInput
import android.app.backup.BackupDataOutput
import android.app.backup.FullBackupDataOutput
import android.content.Context
import android.os.ParcelFileDescriptor
import android.util.Log
import java.io.File

/**
 * 系统自动备份（Auto Backup：Google 云备份 + 设备间迁移 D2D）的**运行期闸门**。默认关、用户可开
 * （设置 → 通用 → 隐私与安全「系统备份」）。
 *
 * # 缺陷来由
 *
 * 清单此前没写 `allowBackup` ⇒ 系统默认开启 Auto Backup：`dataDir/polaris/` 下含节点凭据的 user
 * config 与 Rust 生成的 sing-box 运行配置会随系统备份上传到 Google 账号，也会在换机迁移里被带走，
 * 用户从头到尾不知道。
 *
 * # 为什么是 BackupAgent，不是 `allowBackup="false"`
 *
 * 决策（2026-09-25）：默认关，但允许用户打开。`allowBackup` 是**静态**清单属性，改不了运行期；
 * 且 targetSdk ≥ 31 时它在部分厂商设备上只关云备份、**不关 D2D**（developer.android.com
 * `identity/data/autobackup`）。故清单保留备份能力（`allowBackup="true"`，显式写出），
 * 挂本类 + `fullBackupOnly="true"`（走文件级 Auto Backup，不走 key-value）。
 *
 * # 为什么 `onFullBackup` 一处就能同时管住云备份与 D2D
 *
 * 平台把「这次备份去哪」作为**传输标志**交给同一个入口：`FullBackupDataOutput.getTransportFlags()`
 * 可带 `BackupAgent.FLAG_DEVICE_TO_DEVICE_TRANSFER`（= 2，「直接发往另一台设备，USB / WiFi」）
 * 与 `FLAG_CLIENT_SIDE_ENCRYPTION_ENABLED`（= 1）。平台自己的 `allowBackup` / `dataExtractionRules`
 * 判定也住在 `super.onFullBackup` 里面（AOSP `BackupAgent.java`：先
 * `FullBackup.getBackupScheme(this, mBackupDestination).isFullBackupEnabled(data.getTransportFlags())`，
 * 不过就 `return`）。即：云与 D2D 都经这一个方法；不调 super =
 * 一个文件都不交出去，与平台自己「此目的地禁用」的处置同形。
 * 因此**不另配** `dataExtractionRules` / `fullBackupContent` 兜底：那两份是静态规则，排除凭据
 * 就等于把用户「打开」之后的那条路也一起堵死，与决策矛盾。
 *
 * # 恢复侧不设闸
 *
 * 恢复发生在新设备安装时（此时开关文件还不存在 = 关）。在 `onRestoreFile` 上按开关拦，结果是
 * 「打开备份」永远恢复不回来 —— 功能整条失效。能被恢复的数据只可能来自用户**曾经打开过**开关的那次
 * 备份，故恢复走平台默认实现。开关本身住 `noBackupFilesDir`，不随备份走：恢复后新设备上仍是关，
 * 要继续备份须用户在新设备上重新打开（安全侧默认，不让旧设备上的一次选择静默延续到新设备）。
 * 系统起核的准入摘要同样在 `noBackupFilesDir`，恢复回来的旧运行配置不会被系统拿去不经用户起隧道。
 *
 * # 运行环境
 *
 * 全量备份/恢复时系统以**受限模式**拉起进程：不实例化自定义 `Application`（`PolarisApplication`
 * 不在），故本类只用自己（`BackupAgent` 本身就是 `Context`）读开关，不碰任何进程级单例。
 */
class PolarisBackupAgent : BackupAgent() {

    override fun onFullBackup(data: FullBackupDataOutput) {
        if (!isEnabled(this)) {
            // 关（含开关文件不存在 = 默认）：不调 super ⇒ 本次备份不含任何文件。
            Log.i(TAG, "系统备份开关为关，跳过本次备份（transportFlags=${data.transportFlags}）")
            return
        }
        Log.i(TAG, "系统备份开关为开，走平台默认全量备份（transportFlags=${data.transportFlags}）")
        super.onFullBackup(data)
    }

    // key-value 备份的两个抽象方法：`fullBackupOnly="true"` 下平台不走这条（minSdk 24 ≥ 23）。
    // 写成空实现而不是转调任何东西：万一某个平台版本走到这里，也是什么都不交出去。
    override fun onBackup(
        oldState: ParcelFileDescriptor?,
        data: BackupDataOutput?,
        newState: ParcelFileDescriptor?,
    ) = Unit

    override fun onRestore(data: BackupDataInput?, appVersionCode: Int, newState: ParcelFileDescriptor?) = Unit

    companion object {
        private const val TAG = "PolarisBackup"

        /** 开关文件（存在 ⇔ 开）。住 `noBackupFilesDir`：不随备份走，恢复不会用旧值覆盖本机选择。 */
        private const val SYSTEM_BACKUP_FILE = "system-backup-enabled"

        private fun flagFile(context: Context): File = File(context.noBackupFilesDir, SYSTEM_BACKUP_FILE)

        /** 默认 = 关：文件不存在即关。 */
        fun isEnabled(context: Context): Boolean = flagFile(context).exists()

        fun setEnabled(context: Context, enabled: Boolean) {
            val flag = flagFile(context)
            if (enabled) {
                flag.parentFile?.mkdirs()
                // 先写临时文件再 rename，与 SystemStart 的标记同一写法。
                val tmp = File(flag.parentFile, "${flag.name}.tmp")
                tmp.writeText("1")
                if (!tmp.renameTo(flag)) {
                    tmp.delete()
                    error("android: 写系统备份开关失败：${flag.path}")
                }
            } else if (flag.exists() && !flag.delete()) {
                error("android: 删除系统备份开关失败：${flag.path}")
            }
        }
    }
}
