import java.io.File
import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.logging.LogLevel
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.TaskAction

open class BuildTask : DefaultTask() {
    @Input
    var rootDirRel: String? = null
    @Input
    var target: String? = null
    @Input
    var release: Boolean? = null

    @TaskAction
    fun assemble() {
        val rootDirRel = rootDirRel ?: throw GradleException("rootDirRel cannot be null")
        val target = target ?: throw GradleException("target cannot be null")
        val release = release ?: throw GradleException("release cannot be null")
        val tauriDir = File(project.projectDir, rootDirRel).canonicalFile
        val launcher = project.providers.environmentVariable("POLARIS_TAURI_CLI").orNull
        val command = if (launcher != null) {
            listOf(launcher)
        } else {
            val cli = File(tauriDir, "../ui/node_modules/@tauri-apps/cli/tauri.js").canonicalFile
            if (!cli.isFile) {
                throw GradleException("Tauri CLI is missing at $cli; use scripts/build-android-apk.mjs or install the IDE CLI")
            }
            listOf("node", cli.absolutePath)
        }

        project.exec {
            workingDir(tauriDir)
            commandLine(command + listOf("android", "android-studio-script"))
            if (project.logger.isEnabled(LogLevel.DEBUG)) {
                args("-vv")
            } else if (project.logger.isEnabled(LogLevel.INFO)) {
                args("-v")
            }
            if (release) {
                args("--release")
            }
            args(listOf("--target", target))
        }.assertNormalExitValue()
    }
}
