// polaris-release-facts.gradle.kts —— 行为裁判的**取材点**：只读、不判、不改任何状态。
//
// ── 为什么它不住在 `build.gradle.kts` 里（2026-09-05 收官轮 A2）────────────────────
//
// 这个任务是 `scripts/gate-android-release-behavior.sh` 的唯一事实来源：R8 手里到底有几份
// 规则文件、签名配置里 `storeFile` 指向哪儿、逃生门开没开，四条判据全靠它打出来的行。
//
// 上一版把它写在 `app/build.gradle.kts` 的末尾 —— 也就是**它自己审判的那个文件里**。
// 后果是判据自污染：把取材表达式
//
//     val releaseProguardFiles = android.buildTypes.getByName("release").proguardFiles.toList()
//
// 换成一个写死的三元素清单（或任何别的恒真表达式），裁判的 ⑦ 原样全绿，而 AGP 手里的清单
// 已经变了。**判据的取材源与被判对象是同一份可改的文本时，那条判据没有独立性可言。**
//
// 移出来之后，`build.gradle.kts` 里改任何一行都改不了本文件读的是什么。剩下的那一问
// ——「本文件读的**真的是** AGP 装配好的那份清单吗」—— 由裁判的 ⑨ 用差分实验回答：
// 往 `app/` 放一份新的 `.pro`，AGP 的扫描面随之变化，本任务打出来的清单必须跟着变。
// 打不出来 = 它读的是别的东西（写死的常量、另一个对象），⑨ 当场红。
//
// ── 边界（如实登记）──────────────────────────────────────────────────────────────
//
// · 本文件只**打印**，不判断。判断留在裁判脚本里：构建脚本自己给自己判分与文本判据自污染同形。
// · 逃生门的开关值不在这里重算，而是从 `build.gradle.kts` 经 `extra["releaseFactHatchOpen"]`
//   取生产绑定算出来的那个值。在这里另写一次 `startParameter.projectProperties["…"]`
//   等于给逃生门开第二个读取点：那样一来，把生产读取点改成 `findProperty` 的变异
//   在裁判眼里仍然是绿的（裁判读到的是本文件那份没被改的语义）。
//   取不到那个 extra 属性时 Gradle 直接抛（`extra` 的缺席就是硬失败）—— 静默缺席不可接受。
// · 属性名字面量 `polarisAllowUnsigned` 刻意不写在本文件里，理由同上。
// · proguardFiles 打三处（buildType / defaultConfig / productFlavor）：AGP 是把这三处合并后
//   喂给 R8 的。本文件只负责把三处都摊开，「另外两处必须是空的」那条判断留在裁判的 ⑦b 里。
// · 任务名刻意不以 `polaris` 开头：Gradle 把每个任务名也当作一条 project property 暴露，
//   叫 `polarisXxx` 会让它自己出现在下面那张 `hatchAllPropertyKeys` 表里，
//   把裁判的取材面污染成恒非空。

import com.android.build.api.dsl.ApplicationExtension

val androidExtension = extensions.getByType(ApplicationExtension::class.java)

tasks.register("printPolarisReleaseFacts") {
    // 配置期取值、执行期只打印：取的就是 AGP 此刻手里那份清单本身。
    val releaseProguardFiles = androidExtension.buildTypes.getByName("release").proguardFiles.toList()
    // ── R8 的输入集是**三处的并集**（2026-09-06 终止轮 R2）────────────────────────────
    // AGP 把 `defaultConfig` / 选中的 `productFlavor` / `buildType` 三处的 proguardFiles
    // 合起来喂给 R8（AGP 口径；本仓没在 R8 输入那一层实测过），而上面那一行只取到第三处。
    // 实测到的是取材面这一侧：往 `defaultConfig` 上挂一份
    // `../x.pro`（app/ 的 fileTree 扫描面之外，守卫的「多一份没登记的」也看不见它），
    // 裁判的「集合恰等」原样全绿 —— 本机实测：⑦ 零失败、rc=0。
    // 另外两处今天是空的，但「空」这件事必须每轮实测：把它们打出来交给裁判钉死为空之后，
    // buildType 那一份才**等于**本仓自有规则的全集，⑦ 的恰等才名副其实。
    val defaultConfigProguardFiles = androidExtension.defaultConfig.proguardFiles.toList()
    val flavorProguardFiles = androidExtension.productFlavors
        .flatMap { flavor -> flavor.proguardFiles.map { "${flavor.name}:${it.absolutePath}" } }
        .sorted()
    // 签名配置的 `storeFile`：这是「凭据齐全那条分支」唯一的产物级观察面。
    // AGP 在缺凭据时留一个空壳配置（storeFile == null），凭据齐全时它必须指向一个存在且可读的文件。
    val storeFile = androidExtension.signingConfigs.findByName("release")?.storeFile
    val cmdlineKeys = gradle.startParameter.projectProperties.keys
        .filter { it.startsWith("polaris") }
        .sorted()
    val allKeys = project.properties.keys
        .map { it.toString() }
        .filter { it.startsWith("polaris") }
        .sorted()
    val hatchOpen = project.extra["releaseFactHatchOpen"]
    doLast {
        releaseProguardFiles.forEach { f ->
            println("POLARIS_FACT\tproguardFile\t${f.absolutePath}\t${if (f.isFile) f.length() else -1L}")
        }
        // 空清单也要打印出一个可断言的值：行尾直接结束的话，裁判那边没法用「含某串」表达
        // 「这张表是空的」（grep 的空模式匹配一切）。下面五行都按这条写。
        println(
            "POLARIS_FACT\tproguardFilesDefaultConfig\t" +
                defaultConfigProguardFiles.joinToString(",") { it.absolutePath }
                    .ifEmpty { "<none>" },
        )
        println(
            "POLARIS_FACT\tproguardFilesFlavors\t" +
                flavorProguardFiles.joinToString(",").ifEmpty { "<none>" },
        )
        println("POLARIS_FACT\thatchCmdlineKeys\t${cmdlineKeys.joinToString(",").ifEmpty { "<none>" }}")
        println("POLARIS_FACT\thatchAllPropertyKeys\t${allKeys.joinToString(",").ifEmpty { "<none>" }}")
        println("POLARIS_FACT\thatchOpen\t$hatchOpen")
        // 三段一行：路径、在不在盘上、读不读得动。缺凭据时是 `<none>\tfalse\tfalse`。
        println(
            "POLARIS_FACT\tsigningStoreFile\t${storeFile?.absolutePath ?: "<none>"}" +
                "\t${storeFile?.isFile ?: false}\t${storeFile?.canRead() ?: false}",
        )
    }
}
