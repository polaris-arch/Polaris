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
        // The Android project lives under src-tauri, while the only package.json and
        // installed Tauri CLI live in ui/. Keep src-tauri as the CLI working directory.
        val cli = File(tauriDir, "../ui/node_modules/@tauri-apps/cli/tauri.js").canonicalFile
        if (!cli.isFile) {
            throw GradleException("Tauri CLI is missing at $cli; install the ui/ dependencies first")
        }

        project.exec {
            workingDir(tauriDir)
            executable("node")
            args(cli.absolutePath, "android", "android-studio-script")
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
