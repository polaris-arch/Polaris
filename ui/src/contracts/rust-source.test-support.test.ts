import { describe, expect, it } from 'vitest';

import { maskRustComments, moduleSource, rustFunctionSource } from './rust-source.test-support';

describe('Rust function source boundaries', () => {
  it.each([
    ['spawn_pipe_loggers_with_preopened_files_custodied', 'PreopenedRotatingFile::open(files, generation_bytes, OpenMode::Fresh)'],
    ['spawn_pipe_drainers_custodied', 'let shared = Arc::new(Mutex::new(None));'],
    ['spawn_pipe_readers', 'spawn_pipe_logger(reader, shared);'],
  ])('extracts the actual generic log-budget function %s including its where bounds', (name, body) => {
    const source = moduleSource('crates/log-budget/src/lib');
    const extracted = rustFunctionSource(source, name);
    expect(extracted).toContain(`fn ${name}<O, E>(`);
    expect(extracted).toMatch(/\)\s*(?:-> PipeLogCustody\s*)?where\s+O: Read \+ Send \+ 'static,\s+E: Read \+ Send \+ 'static,\s*\{/);
    expect(extracted).toContain(body);
    expect(extracted.trimEnd().endsWith('}')).toBe(true);
    expect(extracted).not.toContain('fn spawn_pipe_logger(');
    expect(extracted).not.toContain('fn trim_open_file_to_tail(');
  });

  it.each([
    ['crates/core-supervisor/src/admitted_payload/windows/issuer/spawn', 'read_only_stdin', 'FILE_GENERIC_READ'],
    ['crates/core-supervisor/src/admitted_payload/windows', 'spawn_windows_main', 'ExecPurpose::RunMain'],
    ['crates/helper/src/platform/windows/winproc/win', 'native_wait', 'WAIT_TIMEOUT => NativeChildPoll::Running'],
    ['crates/core-supervisor/src/admitted_payload', 'spawn_linux_helper_run', 'command.spawn()?'],
    ['crates/core-supervisor/src/admitted_payload', 'spawn_macos_root_helper_main', 'command.spawn()?'],
  ])('retains the existing plain-function caller %s::%s', (module, name, body) => {
    const extracted = rustFunctionSource(moduleSource(module), name);
    expect(extracted.startsWith(`fn ${name}(`)).toBe(true);
    expect(extracted).toContain(body);
    expect(extracted.trimEnd().endsWith('}')).toBe(true);
  });

  it('balances nested generic bounds, arrows, lifetimes, const expressions and where types', () => {
    const expected = `fn target<'a, T: Trait<Item = Vec<Result<u8, E>>>, F: for<'b> Fn(&'b T) -> Vec<T>, const N: usize = { if 3 > 1 { 4 } else { 2 } }>(
    value: &'a [T; N], map: F,
) -> Result<Vec<<T as Trait>::Item>, E>
where
    F: Fn(&T) -> Vec<T>,
    [u8; { N + 1 }]: Sized,
{
    let nested = { map(&value[0]) };
    Ok(nested)
}`;
    const extracted = rustFunctionSource(`pub async ${expected}\nfn after() { unrelated(); }`, 'target');
    expect(extracted).toBe(expected);
    expect(extracted).not.toContain('unrelated');
  });

  it('balances function-pointer parameter and return groups without treating arrows as generic closes', () => {
    const expected = 'fn target<F: Fn() -> Result<Vec<u8>, E>>(map: F, callback: fn([u8; 2]) -> Vec<u8>) -> fn() -> usize where F: Send { callback([1, 2]); next }';
    expect(rustFunctionSource(`${expected}\nfn after() {}`, 'target')).toBe(expected);
    const plain = 'fn plain(callback: fn(usize) -> Result<[u8; 2], E>) -> (usize, usize) { let _ = callback(1); (1, 2) }';
    expect(rustFunctionSource(plain, 'plain')).toBe(plain);
    const comparisons = 'fn target(value: usize) -> usize { if value < 2 { value << 1 } else { value >> 1 } }';
    expect(rustFunctionSource(comparisons, 'target')).toBe(comparisons);
  });

  it('ignores fake declarations and braces in nested comments and all literal forms', () => {
    const fake = String.raw`
// fn target<A>() { line_trap(); }
/* fn target<B>() { /* nested } */ block_trap(); } */
const TEXT: &str = "fn target<C>() { string_trap(); }";
const RAW: &str = r##"fn target<D>() { raw_trap(); }"##;
const BYTE: &[u8] = b"fn target<E>() { byte_trap(); }";
const RAW_BYTE: &[u8] = br#"fn target<F>() { raw_byte_trap(); }"#;
const CHARACTER: char = '}';
`;
    const expected = 'fn target<T>(value: T) -> T { let text = "} fn after() {"; value }';
    expect(rustFunctionSource(`${fake}${expected}\nfn after() {}`, 'target')).toBe(expected);
    expect(() => rustFunctionSource(fake, 'target')).toThrow('Missing Rust function: target');
  });

  it('keeps byte offsets aligned before and inside Unicode literals and comments', () => {
    const expected = 'fn target<T>(value: T) -> T { let label = "尾😀 } fn fake() {"; value }';
    const source = `const TEXT: &str = "前😀 fn target<U>() { trap(); }"; /* 注释😀 */\n${expected}\nfn after() {}`;
    expect(rustFunctionSource(source, 'target')).toBe(expected);
    const withComment = 'fn target<T>(value: T) -> T { /* 中😀 } */ value }';
    expect(rustFunctionSource(withComment, 'target')).toBe(maskRustComments(withComment));
    expect(() => rustFunctionSource('fn other() {}\nfn 目标target() {}', 'target')).toThrow('Missing Rust function: target');
  });

  it.each([
    "fn target<T>(value: T) -> T where T: Read + Send + 'static { value }",
    'fn target() -> &\'static str { "ready" }',
    "fn target<'a, T>(value: &'a T) -> &'a T where 'a: 'static, T: Send + 'a + 'static { value }",
    "fn target<'寿命, T>(value: &'寿命 T) -> &'寿命 T where T: '寿命 + Send + 'static { value }",
    "fn target(value: &str) -> &'_ str { value }",
  ])('keeps lifetime bounds and borrowed return types within their function: %s', (expected) => {
    const source = `static BEFORE: &str = "before";\n${expected}\nstatic AFTER: &str = "after";\nfn neighbor() { trap(); }`;
    expect(rustFunctionSource(source, 'target')).toBe(expected);
  });

  it.each([
    'fn target<T>() -> T\nstatic LEAKED: T = { trap(); };',
    "fn target<T>() -> T where T: Send + 'static\nstatic LEAKED: T = { trap(); };",
    "fn target() -> &'static str\nstatic LEAKED: &str = { trap(); };",
  ])('still rejects an actual static item after a header/lifetime instead of borrowing its initializer: %s', (source) => {
    expect(() => rustFunctionSource(`${source}\nfn neighbor() {}`, 'target'))
      .toThrow('Unexpected item in Rust function header: target');
  });

  it.each([
    "fn target<T>() -> T where T: Send + ' static { value }\nfn neighbor() { trap(); }",
    "fn target() -> &' { value }\nfn neighbor() { trap(); }",
  ])('rejects a malformed lifetime token without consuming a later body: %s', (source) => {
    expect(() => rustFunctionSource(source, 'target')).toThrow('Invalid Rust function lifetime: target');
  });

  it('skips trait prototypes without borrowing a neighboring function body', () => {
    const prototype = 'trait Consumer { fn target<T>(&self, value: T) -> T where T: Send; }';
    expect(() => rustFunctionSource(`${prototype}\nfn neighbor() { trap(); }`, 'target'))
      .toThrow('Missing Rust function: target');
    const implementation = 'fn target<T>(&self, value: T) -> T where T: Send { value }';
    expect(rustFunctionSource(`${prototype}\nimpl Consumer for Actual { ${implementation} }`, 'target'))
      .toBe(implementation);
    expect(() => rustFunctionSource('fn target_other<T>(value: T) -> T { value }', 'target'))
      .toThrow('Missing Rust function: target');
  });

  it.each([
    'fn target<T: Bound(value: T) { value }\nfn neighbor() { trap(); }',
    'fn target<T; U>(value: T) { value }\nfn neighbor() { trap(); }',
    'fn target<T>>(value: T) { value }',
    'fn target<T>(value: T] { value }\nfn neighbor() { trap(); }',
    'fn target<T>(value: T { value }\nfn neighbor() { trap(); }',
    'fn target<T>() -> Vec<T;\nfn neighbor() { trap(); }',
    'fn target<T>() -> Vec<T>> { value }',
    'fn target<T>() -> { value }',
    'fn target<T>() where { value }',
    'fn target<T>() -> T struct Neighbor { value: T }',
    'fn target<T>() -> T\nfn neighbor() { trap(); }',
    'fn target<T>() { let broken = [1, 2); }',
    'fn target<T>() { if true { value(); }\nfn neighbor() { trap(); }',
  ])('fails closed on a malformed header/body boundary: %s', (source) => {
    expect(() => rustFunctionSource(source, 'target')).toThrow(/Rust function/);
  });

  it('rejects missing and unsafe requested names', () => {
    expect(() => rustFunctionSource('fn other<T>() {}', 'missing')).toThrow('Missing Rust function: missing');
    expect(() => rustFunctionSource('éfn target<T>() {}', 'target')).toThrow('Missing Rust function: target');
    for (const name of ['', 'target.*', 'target<T>', 'target(', 'target\nother']) {
      expect(() => rustFunctionSource('fn target<T>() {}', name)).toThrow('Invalid Rust function name');
    }
  });
});
