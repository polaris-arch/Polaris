import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

// ── rustls-platform-verifier 的 Kotlin 半边（AAR）──────────────────────────────
//
// Rust 侧 reqwest/rustls 在 Android 上的服务端证书校验器是
// `rustls_platform_verifier::Verifier`，它把校验动作转交给 Kotlin 类
// `org.rustls.platformverifier.CertificateVerifier`（经 ClassLoader 反射加载 + JNI 调用）。
// 那个类**不在** crates.io 的 Rust 代码里，而是由 `rustls-platform-verifier-android` crate
// 以预编译 `.aar` 随 crate 分发（上游的取舍见该 crate 的 lib.rs 文档：AGP 版本无法跨项目同步，
// 源码分发不可行；Maven Central 又解决不了「aar 版本跟着 crate 版本走」）。
//
// 少了这一行的后果**不是编译失败**，而是运行期每次 TLS 握手都报证书错（`load_class` 抛
// ClassNotFoundException → 校验器返 Err）—— 比 panic 更难查。Rust 侧 `src/android_tls.rs`
// 的模块文档记着这条。
//
// 路径由 `cargo metadata` 反查而不是写死：crate 版本一变，maven 目录也跟着变，写死等于
// 悄悄用回旧 aar 或直接找不到。下面的 `single()` 让「目录里不是恰好一个 aar」当场报错，
// 而不是随便挑一个。
//
// 为什么不用上游 README 的 `maven { url = ... }` + `implementation("rustls:...:latest.release")`：
// 那条路要 `maven-metadata.xml` 才能解析 `latest.release`，而 crate 里带的是
// `maven-metadata-local.xml`（mavenLocal 的形态）；固定版本号又把版本写死回来了。
// 直接指 aar 文件与本文件已有的 `implementation(files("libs/libbox.aar"))` 同一形态。
@Suppress("UnstableApiUsage")
fun rustlsPlatformVerifierAar(): File {
    // rootDir = gen/android ⇒ 上跳三层是 workspace 根（与本文件底部 `rust { rootDirRel }` 同一基准）。
    val workspaceManifest = File(project.rootDir, "../../../Cargo.toml").canonicalFile
    val metadata = providers.exec {
        commandLine("cargo", "metadata", "--format-version", "1", "--manifest-path", workspaceManifest.path)
    }.standardOutput.asText.get()

    @Suppress("UNCHECKED_CAST")
    val packages = groovy.json.JsonSlurper().parseText(metadata)
        .let { it as Map<String, Any> }["packages"] as List<Map<String, Any>>
    val manifestPath = packages
        .firstOrNull { it["name"] == "rustls-platform-verifier-android" }
        ?.get("manifest_path") as String?
        ?: throw GradleException(
            "cargo metadata 里没有 rustls-platform-verifier-android —— " +
                "它本该由 reqwest 的 rustls-no-provider feature 传递引入，" +
                "见 src-tauri/src/android_tls.rs 模块文档",
        )
    val mavenDir = File(File(manifestPath).parentFile, "maven")
    val aars = mavenDir.walkTopDown().filter { it.isFile && it.extension == "aar" }.toList()
    if (aars.size != 1) {
        throw GradleException("期望 $mavenDir 下恰好一个 .aar，实得 ${aars.size}: $aars")
    }
    return aars.single()
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// ── release 的 R8 规则集：显式收敛，且「少一份」不许静默通过 ──────────────────
//
// 三份 `.pro` 里只有 `proguard-rules.pro` 入库；`proguard-tauri.pro` 与
// `src/main/java/com/polaris2/app/generated/proguard-wry.pro` 由 Tauri CLI 现场生成、被
// `app/.gitignore` 忽略。后两份承载 wry / Tauri 的**全部** JNI keep（`Ipc.postMessage`、
// `RustWebView.evalScript`、`WryActivity.getAppClass` …）。少任何一份，R8 照样跑完、构建照样绿，
// 产物是个白屏包 —— 缺席在这里必须自曝，判据在下方 `gradle.taskGraph.whenReady`。
//
// 扫描面收成「工程根 + `src/` 子树」，不再是 `fileTree(".")`：后者连 `app/build/` 一起扫，
// 且在**配置期**求值 —— 于是「上一次构建跑没跑过」会改变生效的 keep 集，同一份源码能出两种
// release 产物。今天 `find app -name '*.pro'` 恰好三份，收窄前后集合相同。
val proguardRuleFiles: List<File> =
    fileTree(".") { include("*.pro", "src/**/*.pro") }.files.sortedBy { it.path }

// 三份必到（顺序无关，membership 判定）。
val requiredProguardRuleFiles: List<File> = listOf(
    file("proguard-rules.pro"),
    file("proguard-tauri.pro"),
    file("src/main/java/com/polaris2/app/generated/proguard-wry.pro"),
)

// ── release 签名：密钥库在仓外，口令与别名**只**经环境变量 ─────────────────────
//
// 形态由陈先生裁定：密钥库固定 `~/.polaris/android/release.jks`（可用 `POLARIS_KEYSTORE_FILE`
// 覆盖路径，给 CI 与多机用），口令/别名走 `POLARIS_KEYSTORE_PASSWORD` / `POLARIS_KEY_PASSWORD` /
// `POLARIS_KEY_ALIAS`，**绝不落盘**（不生成 `keystore.properties` 之类）。
//
// 🔴 下面三行右边**只能**是裸 `System.getenv(...)`，后面不许接 `?:` / `orElse` / `getOrDefault`：
//    给口令写默认值等于给出一把人人都知道的钥匙，而且它会让「凭据缺失」这件事**再也不会被发现**
//    —— 那正是本批要防的失败形态。这条不是靠自觉：`src-tauri/tests/android_release_signing_wiring.rs`
//    把这三行与下面三处消费点的**形状**逐个取出来做正面断言，加一个回退就红。
//
// 密钥库**路径**允许有默认值（它不是秘密，且默认值就是约定位置）；秘密三件不允许。
val keystoreFile: File = File(
    System.getenv("POLARIS_KEYSTORE_FILE")
        ?: "${System.getProperty("user.home")}/.polaris/android/release.jks",
)
val keystorePasswordFromEnv = System.getenv("POLARIS_KEYSTORE_PASSWORD")
val keyPasswordFromEnv = System.getenv("POLARIS_KEY_PASSWORD")
val keyAliasFromEnv = System.getenv("POLARIS_KEY_ALIAS")

// 缺什么就逐条列出来（失败信息要点名，不能只说「签名没配好」）。
val signingBlockers: List<String> = buildList {
    if (!keystoreFile.isFile) {
        add("密钥库文件不存在：${keystoreFile.path}（用 POLARIS_KEYSTORE_FILE 可改路径）")
    }
    if (keystorePasswordFromEnv.isNullOrEmpty()) add("环境变量 POLARIS_KEYSTORE_PASSWORD 未设置")
    if (keyPasswordFromEnv.isNullOrEmpty()) add("环境变量 POLARIS_KEY_PASSWORD 未设置")
    if (keyAliasFromEnv.isNullOrEmpty()) add("环境变量 POLARIS_KEY_ALIAS 未设置")
}
val releaseSigningReady: Boolean = signingBlockers.isEmpty()

// 显式逃生门：只想量体积、不打算装机时用 `-PpolarisAllowUnsigned=true`。
// 它换来的产物是 AGP 的 `app-<flavor>-release-unsigned.apk` —— 文件名本身就写着没签名。
//
// 🔴 只认**命令行那一次**显式传入。`gradle.startParameter.projectProperties` 装的恰好是本次调用
//    的 `-P` 实参，`gradle.properties` 文件与 `ORG_GRADLE_PROJECT_*` 环境变量都不进这张表。
//
//    换掉原来的 `project.findProperty("polarisAllowUnsigned")` 不是风格问题，是实测出来的洞：
//    那个 API 对三条来源一视同仁，于是往 `gen/android/gradle.properties` 末尾写一行
//    `polarisAllowUnsigned=true`（实测 rc=0、警告行照出），或在 CI 上导出一个
//    `ORG_GRADLE_PROJECT_polarisAllowUnsigned=true`（同样 rc=0），逃生门就**永久且不可见**地
//    开着 —— 此后每一次 release 都静默产出未签名包，构建全绿，没有任何东西会说话。
//    而 `gradle.properties` 今天不在任何一道门的取材面里。
//
//    逃生门的语义是「这一次我知道自己在做什么」，不是「从此以后都不用管」。命令行实参不会被
//    继承到下一次调用，正好把这层语义交给代码持有，而不是交给自觉。
//    判据在 `src-tauri/tests/android_release_signing_wiring.rs`（读取点的形状 + gradle.properties
//    的取材面），两条一起守；本行改回 `findProperty` 就红。
val allowUnsignedRelease: Boolean =
    gradle.startParameter.projectProperties["polarisAllowUnsigned"] == "true"

android {
    compileSdk = 36
    namespace = "com.polaris2.app"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "com.polaris2.app"
        minSdk = 24
        targetSdk = 36
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    sourceSets.getByName("androidTest").assets.srcDir(
        File(project.rootDir, "../../../crates/config-engine/fixtures"),
    )
    signingConfigs {
        create("release") {
            // 凭据齐了才装配。缺凭据时这里保持空壳，由文件末尾的 `gradle.taskGraph.whenReady`
            // 在 release 任务真的进任务图时**大声失败** —— AGP 的默认行为恰恰相反：
            // 它会一声不吭地产出 `app-*-release-unsigned.apk`，而那个包装不上真机。
            if (releaseSigningReady) {
                storeFile = keystoreFile
                storePassword = keystorePasswordFromEnv
                keyAlias = keyAliasFromEnv
                keyPassword = keyPasswordFromEnv
            }
        }
    }
    buildTypes {
        getByName("debug") {
            applicationIdSuffix = ".debug"
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            isMinifyEnabled = true
            proguardFiles(
                *(proguardRuleFiles + getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toTypedArray()
            )
            // 缺凭据时不接线：接上一个空壳 signingConfig 会让 AGP 在执行期报一句语焉不详的
            // 「Keystore file not set」，而不是下面那段点名到变量的失败信息。
            if (releaseSigningReady) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
    // ── Tauri 的 `_up_` 资源树必须进 APK ────────────────────────────────────────
    // AGP 传给 aapt2 的 --ignore-assets 默认串含 `<dir>_*`（= 忽略任何以 `_` 开头的 assets 子目录）。
    // Tauri 把 `bundle.resources` 里每个 `../x` 条目铺成 `assets/_up_/x`，于是**所有以 `../` 写的资源
    // 在 Android 上静默不进包**：构建全绿、tauri 也确实铺好了 assets/_up_/，unzip 里一条都没有。
    // 这里显式重设该串（= 默认串逐条抄回、只删掉 `<dir>_*`），把那一条排除规则去掉。
    // 实测对照：assets/probe-root.txt 与 assets/normaldir/ 进包，assets/_underscore/ 与 assets/_up_/ 不进。
    androidResources {
        ignoreAssetsPatterns.addAll(
            listOf(
                "!.svn", "!.git", "!.gitignore", "!.ds_store", "!*.scc", ".*",
                "!CVS", "!thumbs.db", "!picasa.ini", "!*~",
            )
        )
    }
}

// 凭据齐全那条分支的**读回点**：拿的是 AGP 装配好的那个 SigningConfig 对象本身，
// 不是上面那几个绑定。判据在下方守卫里（`storeFile` 必须存在、可读、且就是 `keystoreFile`）。
val releaseSigningConfig = android.signingConfigs.getByName("release")

// 逃生门开关的**唯一**真值出口：事实任务（polaris-release-facts.gradle.kts）从这里取。
// 在那份文件里另写一次 `startParameter.projectProperties["…"]` 等于给逃生门开第二个读取点，
// 于是把本文件的读取点改回 `findProperty` 的变异在裁判眼里仍然是绿的。
extra["releaseFactHatchOpen"] = allowUnsignedRelease

rust {
    rootDirRel = "../../../"
}

dependencies {
    // sing-box 内核（libbox，gomobile bind 产物）。形态与上游 sing-box-for-android 一致
    // （其 app/build.gradle.kts:184 也是 `implementation(files("libs/libbox.aar"))`）：
    // 上游没有为本仓钉的 v1.14.0 tag 发布预编译 aar，故不能像桌面内核那样 fetch，
    // 由 scripts/build-libbox.sh 现场构建（含 sha256 收据与四个坑的注释）。
    // aar 本体 112 MiB，不入库（gitignore 在 app/.gitignore）。
    implementation(files("libs/libbox.aar"))
    // rustls-platform-verifier 的 Kotlin 校验器（见上方 rustlsPlatformVerifierAar 注释）。
    implementation(files(rustlsPlatformVerifierAar()))
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

// ── 「缺规则文件 / 缺签名凭据」必须大声失败 ──────────────────────────────────
//
// 为什么挂在 `taskGraph.whenReady` 而不是配置期直接 throw：配置期对 **debug** 构建同样求值，
// 那会改掉当前唯一能跑的验收产物的行为。这里只在 release 打包任务真的进了任务图时才开口，
// debug 路径一个字节都不受影响。
gradle.taskGraph.whenReady(
    // 显式 `Action<T>`：`whenReady` 有 Closure / Action 两个重载，裸 lambda 会被 Kotlin 解析到
    // Closure 那个并当场编译失败（实测 AGP 8.11 / Gradle 8.14.3 / Kotlin DSL）。
    // `org.gradle.kotlin.dsl.Action` 这个工厂取的是**带接收者**的 lambda ⇒ `this` 就是任务图。
    Action<org.gradle.api.execution.TaskExecutionGraph> {
        val wantsRelease = allTasks.any { task: org.gradle.api.Task ->
            task.project == project && task.name.endsWith("Release") &&
                (
                    task.name.startsWith("assemble") ||
                        task.name.startsWith("bundle") ||
                        task.name.startsWith("package")
                    )
        }
        if (!wantsRelease) return@Action

        val missingRules = requiredProguardRuleFiles.filter { it !in proguardRuleFiles }
        if (missingRules.isNotEmpty()) {
            throw GradleException(
                "release 的 R8 规则文件缺席：" + missingRules.joinToString { it.path } + "。\n" +
                    "它们由 `tauri android build` 生成、被 app/.gitignore 忽略，承载 wry / Tauri 的" +
                    "全部 JNI keep。缺了它们 R8 会在零 keep 的情况下跑完、构建全绿，而产物白屏。\n" +
                    "先跑一次 `./ui/node_modules/.bin/tauri android build --debug --apk " +
                    "--target aarch64` 把生成物铺出来，再构 release。",
            )
        }

        // ── 「登记之外还多出一份规则文件」同样必须硬失败（集合恰等，不是 membership）──
        //
        // 上一版只判「三份都在」。往 `app/` 放一份没人登记的 `.pro`（`fileTree` 扫得到它），
        // 内容写 `-keep class ** { *; }` 就等于把整个混淆关掉 —— 三份仍然都在，构建全绿，
        // 而 R8 实际吃进去的是四份。规则集是**集合**，多一份与少一份是同一类事故。
        val unregisteredRules = proguardRuleFiles.filter { it !in requiredProguardRuleFiles }
        if (unregisteredRules.isNotEmpty()) {
            throw GradleException(
                "release 的 R8 规则集里有没登记的规则文件：" +
                    unregisteredRules.joinToString { it.path } + "。\n" +
                    "规则集是集合：多一份 `.pro` 就能改掉整包的混淆/剪枝结果（`-keep class ** { *; }` " +
                    "一行就把 R8 关掉了），而「三份都在」这条判定对它是瞎的。\n" +
                    "确实需要它 ⇒ 把它加进本文件的 `requiredProguardRuleFiles`（那张表就是登记表）；" +
                    "不需要 ⇒ 删掉它。",
            )
        }

        if (releaseSigningReady) {
            // ── 凭据齐全这条分支此前**零观测**：`storeFile` 指到哪儿没有任何东西看一眼 ──
            //
            // 最坏的一形不是指向空气（AGP 执行期还会报一句 `Keystore file not set`），
            // 而是指向**另一份存在的**密钥库：构建全绿、包也签得出来，只是签它的不是本工程的密钥。
            // 上面 `signingBlockers` 检查的是 `keystoreFile`（取值侧），这里读的是 AGP 手里那份
            // 装配结果（消费侧）—— 两侧对不上，正是「取值对了、接线接错了」那一形。
            val assembled = releaseSigningConfig.storeFile
            if (assembled == null || !assembled.isFile || !assembled.canRead()) {
                throw GradleException(
                    "Android release 的 signingConfig 里 storeFile 不可用：" +
                        (assembled?.path ?: "<null>") + "\n" +
                        "凭据判定说齐了（" + keystoreFile.path + " 在盘上、三个环境变量都设了），" +
                        "而 AGP 手里那份签名配置指向的东西不存在或读不动 —— 接线与取值对不上。",
                )
            }
            if (assembled.canonicalFile != keystoreFile.canonicalFile) {
                throw GradleException(
                    "Android release 的 signingConfig 里 storeFile 不是本工程认的那一份。\n" +
                        "  AGP 手里：" + assembled.canonicalFile.path + "\n" +
                        "  本工程认：" + keystoreFile.canonicalFile.path + "\n" +
                        "两者不同 = 这个包会被**另一把密钥**签出来，而构建全绿、文件名也不带 " +
                        "`-unsigned`。密钥库路径只该由 POLARIS_KEYSTORE_FILE 决定。",
                )
            }
            return@Action
        }

        val how =
            "缺的是：\n  - " + signingBlockers.joinToString("\n  - ") + "\n\n" +
                "密钥库该放哪：" + keystoreFile.path + "\n" +
                "怎么生成（交互式，keytool 会自己提示输口令；**不要**把口令写成命令行实参 —— " +
                "那会进 shell 历史，也会出现在 ps 的 argv 里）：\n" +
                "  mkdir -p ~/.polaris/android && chmod 700 ~/.polaris/android\n" +
                "  keytool -genkeypair -v -keystore " + keystoreFile.path +
                " -alias <你的别名> -keyalg RSA -keysize 4096 -validity 10000\n\n" +
                "构建前把三件交给环境变量（同样别落盘，读完即用）：\n" +
                "  read -rs -p 'keystore password: ' POLARIS_KEYSTORE_PASSWORD; " +
                "export POLARIS_KEYSTORE_PASSWORD\n" +
                "  read -rs -p 'key password: ' POLARIS_KEY_PASSWORD; export POLARIS_KEY_PASSWORD\n" +
                "  read -p 'key alias: ' POLARIS_KEY_ALIAS; export POLARIS_KEY_ALIAS"

        if (allowUnsignedRelease) {
            logger.lifecycle(
                "警告：-PpolarisAllowUnsigned=true 已生效，本次 release 产物**没有签名**，" +
                    "装不上任何真机（adb install 报 INSTALL_PARSE_FAILED_NO_CERTIFICATES）。\n" +
                    "警告：产物文件名会带 `-unsigned` 后缀，别把它当可分发包。\n" + how,
            )
            return@Action
        }

        throw GradleException(
            "Android release 签名凭据缺失 —— 拒绝静默产出未签名包。\n\n" + how + "\n\n" +
                "只想量体积、不打算装机时，显式走逃生门：-PpolarisAllowUnsigned=true",
        )
    },
)

// ── 行为裁判的取材点：住在**别的文件**里 ────────────────────────────────────────
//
// `printPolarisReleaseFacts`（裁判的唯一事实来源：AGP 手里的 proguardFiles 清单、签名配置的
// storeFile、逃生门的三个观察值）此前就写在本文件末尾 —— 也就是**它自己审判的那个文件里**。
// 把那句取材表达式一换（写死一份三元素清单即可），裁判的 ⑦ 原样全绿而 AGP 手里的清单已经变了。
// 判据的取材源与被判对象是同一份可改的文本时，那条判据没有独立性可言。
//
// 故它搬去 `polaris-release-facts.gradle.kts`：本文件里改任何一行都改不了那边读的是什么。
// 「那边读的**真的是** AGP 装配好的清单吗」由裁判的 ⑨ 用差分实验回答（往 app/ 放一份新的 .pro，
// 打印出来的清单必须跟着变）。删掉下面这一行 ⇒ 任务不存在 ⇒ 裁判每一轮 rc≠0，不会静默。
apply(from = "polaris-release-facts.gradle.kts")

apply(from = "tauri.build.gradle.kts")
