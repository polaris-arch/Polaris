/**
 * W26 跨文件接线门：行为预算在 Rust 单测，这里钉住最容易被后续“局部优化”拆开的四条生产腿。
 */
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { moduleSource, moduleSourceWithTests, rustFunctionSource } from './rust-source.test-support';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../../');
const read = (path: string) => readFileSync(resolve(ROOT, path), 'utf8');

describe('W26 bounded core-log wiring', () => {
  it('runtime never hands a fixed output file back to sing-box', () => {
    // 两条断言的取材面**不同**，不能合成一坨：
    // - `log_file_path: None` 是生产接线 ⇒ 只看生产源码，免得哪天测试夹具里出现同形串把它喂饱；
    // - 那个 `#[test]` 名字是「行为预算真被测了」的凭据 ⇒ 必须看含 tests 的取材面（它现在住在
    //   `runtime/proxy/tests/`，写死 `runtime/proxy.rs` 会把它整个丢掉）。
    expect(moduleSource('src-tauri/src/runtime/proxy')).toContain('log_file_path: None');
    expect(moduleSourceWithTests('src-tauri/src/runtime/proxy')).toContain(
      'runtime_log_output_is_owned_by_bounded_sink_not_core',
    );
  });

  it('all three helper spawners drain pipes through the shared bounded writer', () => {
    // 这两支各带一条**否定**断言（`spawn_pipe_loggers(` 的无界老腿不得回潮），所以取的是
    // 模块生产面而不是写死 `<模块>.rs`：spawner 拆出生产子模块时，写死单文件的否定断言会
    // 静默恒真 —— 门还在、报告还是绿的，判据已经没了。`tests/` 必须剔除（夹具里的同形串会假红）。
    for (const module of [
      'crates/helper/src/platform/macos/server',
      'crates/helper/src/platform/windows/winproc/win',
    ]) {
      const source = moduleSource(module);
      expect(source, module).toContain('preopen_log_files');
      expect(source, module).toContain(
        'polaris_log_budget::spawn_pipe_loggers_with_preopened_files_custodied(',
      );
      expect(source, module).toContain('polaris_log_budget::DEFAULT_GENERATION_BYTES');
      expect(source, module).toContain('polaris_log_budget::spawn_pipe_drainers_custodied(');
      expect(source, module).not.toContain('polaris_log_budget::spawn_pipe_loggers(');
      expect(source, module).not.toContain('polaris_log_budget::spawn_pipe_loggers_with_preopened_files(');
      expect(source, module).not.toContain('polaris_log_budget::spawn_pipe_drainers(');
    }

    // 返回的 writer custody 必须跟原 birth/HANDLE 一起持有；仅调用有界 API 不足以守住迟到写入。
    const macos = moduleSource('crates/helper/src/platform/macos/server');
    expect(macos).toContain('birth.attach_log_custody(log_custody);');
    const windows = moduleSource('crates/helper/src/platform/windows/winproc/win');
    // Windows now creates native inherited pipes inside its execution factory.
    const windowsExecution = moduleSource('crates/core-supervisor/src/admitted_payload/windows');
    const windowsIssuer = moduleSource('crates/core-supervisor/src/admitted_payload/windows/issuer/spawn');
    expect(rustFunctionSource(windowsIssuer, 'pipe')).toContain('CreatePipe(&mut r, &mut w, &sa, 0)');
    const suspended = rustFunctionSource(windowsIssuer, 'create_suspended');
    expect(suspended).toMatch(/let \(stdout, out_write\) = pipe\(\)\?;\s*let \(stderr, err_write\) = pipe\(\)\?;/);
    expect(suspended).toContain('for f in [config, &out_write, &err_write]');
    expect(suspended).toMatch(/dwFlags = STARTF_USESTDHANDLES;\s*startup.StartupInfo.hStdInput = handles\[0\];\s*startup.StartupInfo.hStdOutput = handles\[1\];\s*startup.StartupInfo.hStdError = handles\[2\];/);
    expect(rustFunctionSource(windowsIssuer, 'spawn')).toMatch(/stdout: suspended.stdout.take\(\).expect\("stdout"\),\s*stderr: suspended.stderr.take\(\).expect\("stderr"\),/);
    expect(rustFunctionSource(windowsExecution, 'spawn_windows_main')).toContain('issuer::spawn(&self.payload.protected.inner, config, cwd)');
    expect(rustFunctionSource(windowsExecution, 'into_parts')).toMatch(/self.stdout,\s*self.stderr,\s*self.custody,/);
    const tracked = rustFunctionSource(windows, 'spawn_tracked');
    expect(tracked).toContain('let child = execution.spawn_windows_main(file, Path::new(&cwd))?;');
    expect(tracked).toMatch(/let \(pid, handle, stdout, stderr, execution_custody\) = child.into_parts\(\);\s*let stdout = Some\(stdout\);\s*let stderr = Some\(stderr\);/);
    expect(tracked).toMatch(/spawn_pipe_loggers_with_preopened_files_custodied\(\s*stdout,\s*stderr,/);
    expect(windows).toMatch(/custody\s*\.as_mut\(\)\s*\.expect\("published owned HANDLE"\)\s*\.log_custody = Some\(log_custody\);/);
    expect(windows).toContain('if log_custody.revoke().is_err()');

    const linux = read('crates/helper/src/platform/linux/server.rs');
    expect(linux).toContain('polaris_log_budget::spawn_pipe_loggers_with_file(');
    const linuxFactory = rustFunctionSource(read('crates/core-supervisor/src/admitted_payload.rs'), 'spawn_linux_helper_run');
    expect(linuxFactory).toMatch(/if pipe \{\s*command\s*\.stdout\(std::process::Stdio::piped\(\)\)\s*\.stderr\(std::process::Stdio::piped\(\)\);/);
    const verified = rustFunctionSource(linux, 'spawn_verified');
    expect(verified).toMatch(/let mut child = execution\s*\.spawn_linux_helper_run\([\s\S]*?pinned_log.is_some\(\),/);
    for (const stream of ['stdout', 'stderr']) {
      expect(verified).toContain(`let ${stream} = child.take_${stream}().map(|inner| PinnedReader {`);
    }
    expect(verified).toMatch(/spawn_pipe_loggers_with_file\(\s*stdout,\s*stderr,\s*&owned_log_path,/);
    expect(linux).toContain('std::os::unix::fs::fchown(file, Some(uid), Some(gid))');

    const budget = read('crates/log-budget/src/lib.rs');
    expect(budget).toContain('after_open(writer.as_ref().map(|opened| &opened.file))');
    expect(budget).toContain('PreopenedRotatingFile::open(files, generation_bytes, OpenMode::Fresh)');
    expect(budget).toContain('PipeLogCustody { writer: shared }');
  });

  it('Mac factory pipes reach the same custodied bounded sink without losing their owners', () => {
    const sources = {
      server: moduleSource('crates/helper/src/platform/macos/server'),
      execution: read('crates/core-supervisor/src/admitted_payload.rs'),
      command: moduleSource('crates/core-supervisor/src/admitted_payload/macos'),
    };
    const compact = (source: string) => source.replace(/\s+/g, '');
    const wired = (input: typeof sources): boolean => {
      const spawn = compact(rustFunctionSource(input.server, 'do_native_spawn'));
      const factory = compact(rustFunctionSource(input.execution, 'spawn_macos_root_helper_main'));
      const command = compact(rustFunctionSource(input.command, 'configure_main_command'));
      const child = input.execution.slice(input.execution.indexOf('impl StdCustodiedChild {'));
      return spawn.includes('letexecution=core.prepare(polaris_core_supervisor::admitted_payload::ExecPurpose::RunMain)')
        && /letmutchild=execution\.spawn_macos_root_helper_main\(Path::new\(&params.cfg\),Path::new\(conf_dir\),log_files.is_some\(\),/.test(spawn)
        && spawn.includes('letstdout=child.take_stdout();')
        && spawn.includes('letstderr=child.take_stderr();')
        && spawn.includes('spawn_pipe_loggers_with_preopened_files_custodied(stdout,stderr,files,polaris_log_budget::DEFAULT_GENERATION_BYTES,)')
        && spawn.includes('spawn_pipe_drainers_custodied(stdout,stderr)')
        && spawn.includes('birth.attach_log_custody(log_custody);')
        && factory.includes('macos::configure_main_command(&mutcommand,config,working_directory,pipe);')
        && factory.includes('letchild=command.spawn()?;')
        && factory.includes('child:Some(child)')
        && /ifpipe\{command\.stdout\(std::process::Stdio::piped\(\)\)\.stderr\(std::process::Stdio::piped\(\)\);\}/.test(command)
        && ['stdout', 'stderr'].every(stream => {
          const take = compact(rustFunctionSource(child, `take_${stream}`));
          return take.includes(`child.${stream}.take().map(|reader|OwnedReader{`)
            && take.includes('_owner:Arc::clone(self.owner.as_ref().expect(')
            && take.includes('_lease_owner:self.lease_owner.as_ref().cloned(),');
        });
    };
    expect(wired(sources)).toBe(true);
    const negatives = [
      { ...sources, execution: sources.execution.replace('working_directory, pipe);', 'working_directory, false);') },
      { ...sources, command: sources.command.replace('.stderr(std::process::Stdio::piped())', '.stderr(std::process::Stdio::null())') },
      { ...sources, server: sources.server.replace('let stdout = child.take_stdout();', 'let stdout = None;') },
      { ...sources, server: sources.server.replace('spawn_pipe_loggers_with_preopened_files_custodied(', 'spawn_pipe_loggers_with_preopened_files(') },
    ];
    for (const input of negatives) {
      expect(input).not.toEqual(sources);
      expect(wired(input)).toBe(false);
    }
  });

  it('legacy log is surfaced and only archived/deleted after an explicit user action', () => {
    const commands = read('src-tauri/src/commands/misc/logs.rs');
    const archive = commands.indexOf('fn archive_legacy_log(');
    const copy = commands.indexOf('std::fs::copy(source, &temporary)', archive);
    const sync = commands.indexOf('.and_then(|file| file.sync_all())', copy);
    const commit = commands.indexOf('std::fs::rename(&temporary, destination)', sync);
    const remove = commands.indexOf('std::fs::remove_file(source)', commit);
    expect([archive, copy, sync, commit, remove].every((position) => position >= 0)).toBe(true);
    expect(archive).toBeLessThan(copy);
    expect(copy).toBeLessThan(sync);
    expect(sync).toBeLessThan(commit);
    expect(commit).toBeLessThan(remove);

    const screen = read('ui/src/components/screens/logs/LogsScreen.tsx');
    expect(screen).toContain('.legacyInfo()');
    expect(screen).toContain('.archiveLegacy()');
    expect(screen).toContain('.deleteLegacy()');
    expect(screen).toContain("confirmTwice(DELETE_LEGACY_KEY");
    expect(screen).toContain("t('logs.legacyBody'");
    expect(screen).toContain("t('logs.archiveLegacy')");
    expect(screen).toContain("t('logs.deleteLegacy')");
    expect(commands).toContain('key::NATIVE_LOG_FILE_TYPE');

    const deleteCommand = commands.slice(
      commands.indexOf('pub fn logs_delete_legacy('),
      commands.indexOf('fn delete_legacy_log('),
    );
    expect(deleteCommand).toContain('state.config().dir().join(LEGACY_SINGBOX_LOG)');
    expect(deleteCommand).not.toMatch(/(?:path|source)\s*:\s*(?:String|PathBuf)/);
  });
});
