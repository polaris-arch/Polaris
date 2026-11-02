/**
 * W26 跨文件接线门：行为预算在 Rust 单测，这里钉住最容易被后续“局部优化”拆开的四条生产腿。
 */
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { maskRustComments, moduleSource, moduleSourceWithTests, rustFunctionSource } from './rust-source.test-support';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../../');
const read = (path: string) => readFileSync(resolve(ROOT, path), 'utf8');

type WindowsLogSources = { issuer: string; execution: string; helper: string; budget: string };

function windowsLogSources(): WindowsLogSources {
  return {
    issuer: maskRustComments(moduleSource('crates/core-supervisor/src/admitted_payload/windows/issuer/spawn')),
    execution: maskRustComments(moduleSource('crates/core-supervisor/src/admitted_payload/windows')),
    helper: maskRustComments(moduleSource('crates/helper/src/platform/windows/winproc/win')),
    budget: maskRustComments(read('crates/log-budget/src/lib.rs')),
  };
}

// Function-scoped, ordered data-flow anchors. The real native wait and the
// writer-mutex revocation are separate facts: readers may continue to EOF after
// revoke, but cannot write to a retired/replacement session. No EOF is a Child
// exit receipt, and no successful TerminateProcess call substitutes for wait.
function windowsLogWiring(input: WindowsLogSources): boolean {
  const compact = (source: string) => source.replace(/\s+/g, '');
  const fn = (source: string, name: string) => compact(rustFunctionSource(source, name));
  const drop = (type: string) => {
    const marker = `impl Drop for ${type} {`;
    const start = input.issuer.indexOf(marker);
    if (start < 0) throw new Error(`Missing ${marker}`);
    return fn(input.issuer.slice(start), 'drop');
  };
  try {
    const pipe = fn(input.issuer, 'pipe');
    const stdin = fn(input.issuer, 'read_only_stdin');
    const duplicate = fn(input.issuer, 'duplicate');
    const suspended = fn(input.issuer, 'create_suspended');
    const spawn = fn(input.issuer, 'spawn');
    const factory = fn(input.execution, 'spawn_windows_main');
    const parts = fn(input.execution, 'into_parts');
    const tracked = fn(input.helper, 'spawn_tracked');
    const poll = fn(input.helper, 'poll_native_child');
    const wait = fn(input.helper, 'native_wait');
    const retire = fn(input.helper, 'retire_native_child');
    const logger = fn(input.budget, 'spawn_pipe_loggers_with_preopened_files_custodied');
    const drainer = fn(input.budget, 'spawn_pipe_drainers_custodied');
    const readers = fn(input.budget, 'spawn_pipe_readers');
    const reader = fn(input.budget, 'spawn_pipe_logger');
    const revoke = fn(input.budget, 'revoke');
    return /bInheritHandle:0,\};check\(unsafe\{CreatePipe\(&mutr,&mutw,&sa,0\)\}\)\?;Ok\(\(File::from\(owned\(r\)\?\),File::from\(owned\(w\)\?\)\)\)/.test(pipe)
      && /fnread_only_stdin\(f:&File\)->io::Result<OwnedHandle>\{letmuth=ptr::null_mut\(\);check\(unsafe\{DuplicateHandle\(GetCurrentProcess\(\),f.as_raw_handle\(\),GetCurrentProcess\(\),&muth,FILE_GENERIC_READ,1,0,\)\}\)\?;owned\(h\)\}$/.test(stdin)
      && /fnduplicate\(f:&File\)->io::Result<OwnedHandle>\{letmuth=ptr::null_mut\(\);check\(unsafe\{DuplicateHandle\(GetCurrentProcess\(\),f.as_raw_handle\(\),GetCurrentProcess\(\),&muth,0,1,DUPLICATE_SAME_ACCESS,\)\}\)\?;owned\(h\)\}$/.test(duplicate)
      && /let\(stdout,out_write\)=pipe\(\)\?;let\(stderr,err_write\)=pipe\(\)\?;letmutinherit=vec!\[read_only_stdin\(config\)\?\];forfin\[&out_write,&err_write\]\.into_iter\(\)\.chain\(guards.iter\(\).copied\(\)\)\{inherit.push\(duplicate\(f\)\?\);\}lethandles:Vec<_>=inherit.iter\(\).map\(AsRawHandle::as_raw_handle\).collect\(\);/.test(suspended)
      && /attributes.set\(PROC_THREAD_ATTRIBUTE_HANDLE_LIST,&handles\)\?;attributes.set\(PROC_THREAD_ATTRIBUTE_JOB_LIST,&jobs\)\?;/.test(suspended)
      && /dwFlags=STARTF_USESTDHANDLES;startup.StartupInfo.hStdInput=handles\[0\];startup.StartupInfo.hStdOutput=handles\[1\];startup.StartupInfo.hStdError=handles\[2\];startup.lpAttributeList=attributes.pointer;/.test(suspended)
      && /CreateProcessW\(application.as_ptr\(\),command.as_mut_ptr\(\),ptr::null\(\),ptr::null\(\),1,CREATE_SUSPENDED\|EXTENDED_STARTUPINFO_PRESENT\|CREATE_UNICODE_ENVIRONMENT\|CREATE_NO_WINDOW\|CREATE_NEW_PROCESS_GROUP,environment.as_ptr\(\).cast\(\),cwd.as_ptr\(\),&startup.StartupInfo,&mutinfo,\)/.test(suspended)
      && /\)\}\)\?;letprocess=owned\(info.hProcess\)\?;letthread=owned\(info.hThread\)\?;drop\(inherit\);drop\(out_write\);drop\(err_write\);Ok\(Suspended\{process:Some\(process\),thread,job:Some\(job\),stdout:Some\(stdout\),stderr:Some\(stderr\),pid:info.dwProcessId,\}\)/.test(suspended)
      && /letguards:Vec<&File>=owner.files.iter\(\).map\(\|\(_,f,_\)\|f\)\.chain\(owner.directories.iter\(\)\)\.chain\(owner.parents.iter\(\)\)\.chain\(owner.system.iter\(\)\)\.chain\(std::iter::once\(&owner.occupancy\)\)\.collect\(\);letmutsuspended=create_suspended\(&owner.final_exe,&owner.final_sidecar,&config,cwd,&guards,purpose,\)\?;/.test(spawn)
      && /stdout:suspended.stdout.take\(\),stderr:suspended.stderr.take\(\),custody,pid:suspended.pid,/.test(spawn)
      && /_job:suspended.job.take\(\).expect\("atomicjob"\),cohort:Some\(Arc::clone\(owner\)\),/.test(spawn)
      && /issuer::spawn\(&self.payload.protected.inner,config,cwd,ExecPurpose::RunMain,\)/.test(factory)
      && /\(self.pid,self.process,self.stdout.expect\("mainstdout"\),self.stderr.expect\("mainstderr"\),self.custody,\)/.test(parts)
      && /letchild=execution.spawn_windows_main\(file,Path::new\(&cwd\)\)\?;letprocess_ms=crate::elapsed_ms\(process_started\);let\(pid,handle,stdout,stderr,execution_custody\)=child.into_parts\(\);letstdout=Some\(stdout\);letstderr=Some\(stderr\);/.test(tracked)
      && /log_custody:None,_execution_custody:execution_custody,/.test(tracked)
      && /letlog_custody=ifletSome\(files\)=log_files\{polaris_log_budget::spawn_pipe_loggers_with_preopened_files_custodied\(stdout,stderr,files,polaris_log_budget::DEFAULT_GENERATION_BYTES,\)\}else\{polaris_log_budget::spawn_pipe_drainers_custodied\(stdout,stderr\)\};custody.as_mut\(\).expect\("publishedownedHANDLE"\).log_custody=Some\(log_custody\);/.test(tracked)
      && /c.pid==target.pid.get\(\)&&c.birth==Some\(target.birth\)/.test(poll)
      && /native_wait\(child,0\)/.test(poll)
      && /matchunsafe\{WaitForSingleObject\(child.handle.as_raw_handle\(\).cast\(\),timeout_ms\)\}\{WAIT_OBJECT_0=>\{child.exit_cached=true;NativeChildPoll::Exited\}WAIT_TIMEOUT=>NativeChildPoll::Running,_=>NativeChildPoll::Unknown,\}/.test(wait)
      && /c.pid==target.pid.get\(\)&&c.birth==Some\(target.birth\)&&c.exit_cached/.test(retire)
      && /letSome\(log_custody\)=guard.as_ref\(\).and_then\(\|child\|child.log_custody.as_ref\(\)\)else\{returnfalse;\};iflog_custody.revoke\(\).is_err\(\)\{returnfalse;\}\*guard=None;true/.test(retire)
      && (retire.match(/\*guard=None;/g) ?? []).length === 1
      && /ifunsafe\{WaitForSingleObject\(self.process.as_raw_handle\(\),0\)\}!=WAIT_OBJECT_0\{ifletSome\(owner\)=self.cohort.take\(\)\{let_=Arc::into_raw\(owner\);\}\}/.test(drop('ChildCustody'))
      && /ifunsafe\{WaitForSingleObject\(process.as_raw_handle\(\),0\)\}!=WAIT_OBJECT_0\{ifletSome\(job\)=self.job.take\(\)\{drop\(job\);\}unsafe\{WaitForSingleObject\(process.as_raw_handle\(\),10_000\);\}\}/.test(drop('Suspended'))
      && /letwriter=PreopenedRotatingFile::open\(files,generation_bytes,OpenMode::Fresh\)\.map\(PipeLogWriter::Preopened\)\.ok\(\);letshared=Arc::new\(Mutex::new\(writer\)\);spawn_pipe_readers\(stdout,stderr,Arc::clone\(&shared\)\);PipeLogCustody\{writer:shared\}/.test(logger)
      && /letshared=Arc::new\(Mutex::new\(None\)\);spawn_pipe_readers\(stdout,stderr,Arc::clone\(&shared\)\);PipeLogCustody\{writer:shared\}/.test(drainer)
      && /ifletSome\(reader\)=stdout\{spawn_pipe_logger\(reader,Arc::clone\(&shared\)\);\}ifletSome\(reader\)=stderr\{spawn_pipe_logger\(reader,shared\);\}/.test(readers)
      && /letmutbuf=\[0_u8;16\*1024\];loop\{letOk\(read\)=reader.read\(&mutbuf\)else\{return;\};ifread==0\{return;\}ifletOk\(mutguard\)=writer.lock\(\)\{ifletSome\(file\)=guard.as_mut\(\)\{iffile.write_chunk\(&buf\[\.\.read\]\)\.is_err\(\)\{\*guard=None;\}\}\}\}/.test(reader)
      && /letmutwriter=self.writer.lock\(\)\.map_err\(\|_\|std::io::Error::other\("pipelogwritercustodyunavailable"\)\)\?;\*writer=None;Ok\(\(\)\)/.test(revoke);
  } catch {
    return false; // A missing/renamed function or ownership scope is a broken wire.
  }
}

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
    // Read-only stdin, the complete inherited guard list and the actual
    // native-wait -> writer-revoke chain are all required, not just pipe tokens.
    expect(windowsLogWiring(windowsLogSources())).toBe(true);
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

  it('Windows log wiring rejects lost inheritance, custody, drain and wait links', () => {
    const source = windowsLogSources();
    const change = (key: keyof WindowsLogSources, name: string, before: string, after: string): WindowsLogSources => {
      const body = rustFunctionSource(source[key], name);
      expect(body, `${key}::${name} mutation anchor`).toContain(before);
      return { ...source, [key]: source[key].replace(body, body.replace(before, after)) };
    };
    const negatives: Array<[string, WindowsLogSources]> = [
      ['stdin inherits parent DELETE rights', change('issuer', 'read_only_stdin', 'FILE_GENERIC_READ,', 'DUPLICATE_SAME_ACCESS,')],
      ['ordinary pipe inherits before the restricted list', change('issuer', 'pipe', 'bInheritHandle: 0,', 'bInheritHandle: 1,')],
      ['stderr writer omitted', change('issuer', 'create_suspended', '[&out_write, &err_write]', '[&out_write]')],
      ['physical guards omitted', change('issuer', 'create_suspended', '.chain(guards.iter().copied())', '')],
      ['handle list not applied', change('issuer', 'create_suspended', 'PROC_THREAD_ATTRIBUTE_HANDLE_LIST', 'PROC_THREAD_ATTRIBUTE_PARENT_PROCESS')],
      ['stderr points at stdout', change('issuer', 'create_suspended', 'hStdError = handles[2]', 'hStdError = handles[1]')],
      ['parent duplicate writer leaks and prevents EOF', change('issuer', 'create_suspended', 'drop(inherit);', '')],
      ['parent original stdout writer leaks', change('issuer', 'create_suspended', 'drop(out_write);', '')],
      ['parent original stderr writer leaks', change('issuer', 'create_suspended', 'drop(err_write);', '')],
      ['system dependency guard omitted', change('issuer', 'spawn', '.chain(owner.system.iter())', '')],
      ['occupancy guard omitted', change('issuer', 'spawn', '.chain(std::iter::once(&owner.occupancy))', '')],
      ['stdout reader discarded by factory', change('issuer', 'spawn', 'stdout: suspended.stdout.take(),', 'stdout: None,')],
      ['native factory purpose changed', change('execution', 'spawn_windows_main', 'ExecPurpose::RunMain,', 'ExecPurpose::CheckConfig,')],
      ['stderr reader discarded in conversion', change('execution', 'into_parts', 'self.stderr.expect("main stderr")', 'self.stdout.expect("main stdout")')],
      ['factory custody not retained', change('helper', 'spawn_tracked', '_execution_custody: execution_custody,', '_execution_custody: drop(execution_custody),')],
      ['unbounded logger substituted', change('helper', 'spawn_tracked', 'spawn_pipe_loggers_with_preopened_files_custodied(', 'spawn_pipe_loggers(')],
      ['fallback does not drain', change('helper', 'spawn_tracked', 'spawn_pipe_drainers_custodied(stdout, stderr)', 'drop((stdout, stderr))')],
      ['stderr does not reach readers', change('budget', 'spawn_pipe_readers', 'spawn_pipe_logger(reader, shared);', 'drop((reader, shared));')],
      ['write failure stops draining', change('budget', 'spawn_pipe_logger', '*guard = None;', '*guard = None; return;')],
      ['reader no longer stops at EOF', change('budget', 'spawn_pipe_logger', 'if read == 0', 'if read == usize::MAX')],
      ['revoke retains writer', change('budget', 'revoke', '*writer = None;', '')],
      ['timeout falsely claims process exit', change('helper', 'native_wait', 'WAIT_TIMEOUT => NativeChildPoll::Running', 'WAIT_TIMEOUT => NativeChildPoll::Exited')],
      ['retire does not require actual exit', change('helper', 'retire_native_child', '&& c.exit_cached', '')],
      ['failed revoke still releases custody', change('helper', 'retire_native_child', 'if log_custody.revoke().is_err()', 'if false')],
      ['failed suspended child does not close its fail-stop job', { ...source, issuer: source.issuer.replace('drop(job);', 'std::mem::forget(job);') }],
      ['failed suspended child does not wait on its retained HANDLE', { ...source, issuer: source.issuer.replace('WaitForSingleObject(process.as_raw_handle(), 10_000);', 'WaitForSingleObject(process.as_raw_handle(), 0);') }],
      ['unknown child cohort is released', { ...source, issuer: source.issuer.replace('let _ = Arc::into_raw(owner);', 'drop(owner);') }],
    ];
    for (const [label, input] of negatives) {
      expect(input, `${label}: mutation must change real source`).not.toEqual(source);
      expect(windowsLogWiring(input), label).toBe(false);
    }
    // Comments cannot compensate for a broken data-flow link.
    const missing = change('issuer', 'create_suspended', '[&out_write, &err_write]', '[&out_write]');
    missing.issuer = maskRustComments(`${missing.issuer}\n// [&out_write, &err_write]\n`);
    expect(windowsLogWiring(missing)).toBe(false);
  });

  it('Mac factory pipes reach the same custodied bounded sink without losing their owners', () => {
    const sources = {
      server: moduleSource('crates/helper/src/platform/macos/server'),
      execution: maskRustComments(read('crates/core-supervisor/src/admitted_payload.rs')),
      command: moduleSource('crates/core-supervisor/src/admitted_payload/macos'),
    };
    const compact = (source: string) => source.replace(/\s+/g, '');
    const wired = (input: typeof sources): boolean => {
      const admittedEntry = compact(rustFunctionSource(input.server, 'do_native_spawn'));
      const spawn = compact(rustFunctionSource(input.server, 'do_native_spawn_admitted'));
      const factory = compact(rustFunctionSource(input.execution, 'spawn_macos_root_helper_main'));
      const command = compact(rustFunctionSource(input.command, 'configure_main_command'));
      const child = input.execution.slice(input.execution.indexOf('impl StdCustodiedChild {'));
      // The admission wrapper delegates to the same production spawn/pipe body.
      return admittedEntry.includes('polaris_core_supervisor::config_gate::with_check_admission(||{self.do_native_spawn_admitted(params,core,conf_dir,uid)})')
        && spawn.includes('letexecution=core.prepare(polaris_core_supervisor::admitted_payload::ExecPurpose::RunMain)')
        && /letmutchild=execution\.spawn_macos_root_helper_main\(Path::new\(&params.cfg\),Path::new\(conf_dir\),log_files.is_some\(\),/.test(spawn)
        && spawn.includes('letstdout=child.take_stdout();')
        && spawn.includes('letstderr=child.take_stderr();')
        && spawn.includes('spawn_pipe_loggers_with_preopened_files_custodied(stdout,stderr,files,polaris_log_budget::DEFAULT_GENERATION_BYTES,)')
        && spawn.includes('spawn_pipe_drainers_custodied(stdout,stderr)')
        && spawn.includes('birth.attach_log_custody(log_custody);')
        && factory.includes('macos::configure_main_command(&mutcommand,config,working_directory,pipe);')
        && factory.includes('letchild=command.spawn()?;')
        && factory.includes('child:Some(child)')
        && factory.includes('owner:Some(self.payload)')
        && factory.includes('lease_owner:Some(lease_owner)')
        && /ifpipe\{command\.stdout\(std::process::Stdio::piped\(\)\)\.stderr\(std::process::Stdio::piped\(\)\);\}/.test(command)
        && ['stdout', 'stderr'].every(stream => {
          const take = compact(rustFunctionSource(child, `take_${stream}`));
          return take.includes(`child.${stream}.take().map(|reader|OwnedReader{`)
            && take.includes('_owner:Arc::clone(self.owner.as_ref().expect(')
            && take.includes('_lease_owner:self.lease_owner.as_ref().cloned(),');
        });
    };
    expect(wired(sources)).toBe(true);
    const factorySource = rustFunctionSource(sources.execution, 'spawn_macos_root_helper_main');
    const negatives = [
      { ...sources, server: sources.server.replace('self.do_native_spawn_admitted(params, core, conf_dir, uid)', 'self.do_native_spawn_without_admission(params, core, conf_dir, uid)') },
      { ...sources, server: sources.server.replace('config_gate::with_check_admission(|| {', 'config_gate::without_check_admission(|| {') },
      { ...sources, execution: sources.execution.replace(factorySource, factorySource.replace('owner: Some(self.payload),', 'owner: None,')) },
      { ...sources, execution: sources.execution.replace(factorySource, factorySource.replace('lease_owner: Some(lease_owner),', 'lease_owner: None,')) },
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
