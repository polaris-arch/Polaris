import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { SOURCE_TESTS, BOOTSTRAP_TESTS } from './run-native-dns-controlled-tests.mjs';
import { TESTS as LOOPBACK_TESTS } from './runtime-config-loopback.mjs';

// gate-rust.sh 与 ci.yml 的 Rust 门必须逐字一致——起因见 gate-rust.sh 头注：本机手跑门用的
// 命令曾比 CI 弱，且这个仓已经被「判据取材面吃到注释里的同名字样」打脸过三次，这里不能再来一次。
//
// 取材两步走：
//   1. stripComments 剥掉 YAML/bash 共用的 `#` 注释（整行注释、引号外的行内注释都剥），
//      不剥引号内的 `#`（本仓两个文件都不出现这种字符，但函数按通用规则实现，不押注特例）。
//   2. 从剥完注释的文本里分别抠出 5 个门各自的「env 变量 + run/命令」，两侧独立抠取后互相比对——
//      不是各自比对一份写死在测试里的期望字符串。这样改 ci.yml 不改脚本、或改脚本不改 ci.yml，
//      两个方向都会让抠出来的值不相等而变红（下面「双向变红」两个 test 用变异真实验证这一点）。

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');

function stripComments(text) {
  return text
    .split('\n')
    .map((line) => {
      if (line.trimStart().startsWith('#')) return '';
      let inSingle = false;
      let inDouble = false;
      for (let i = 0; i < line.length; i++) {
        const ch = line[i];
        if (ch === "'" && !inDouble) inSingle = !inSingle;
        else if (ch === '"' && !inSingle) inDouble = !inDouble;
        else if (ch === '#' && !inSingle && !inDouble) return line.slice(0, i);
      }
      return line;
    })
    .join('\n');
}

/** Read one named step, bounded by every sibling step or the end of its job. */
function extractCiStepBlock(strippedYaml, stepName) {
  const lines = strippedYaml.split('\n');
  const startIdx = lines.findIndex((l) => l.trim() === `- name: ${stepName}`);
  assert.ok(startIdx !== -1, `ci.yml 里找不到步骤 "- name: ${stepName}"`);
  let endIdx = lines.length;
  for (let i = startIdx + 1; i < lines.length; i++) {
    if (/^ {6}- |^ {0,4}\S/.test(lines[i])) {
      endIdx = i;
      break;
    }
  }
  return lines.slice(startIdx, endIdx).join('\n');
}

function extractRunCommand(block, context) {
  const m = block.match(/^\s*run:\s*(.+)$/m);
  assert.ok(m, `${context}：块里找不到 run: 命令\n---\n${block}\n---`);
  return m[1].trim();
}

function extractEnvVar(block, varName) {
  const m = block.match(new RegExp(`${varName}:\\s*"([^"]*)"`));
  return m ? m[1] : null;
}

/** 抠出 gate-rust.sh 里 `run_gate <name> ...` 这一整行（已剥注释）。 */
function extractShellGateLine(strippedShell, gateName) {
  const m = strippedShell.match(new RegExp(`^run_gate ${gateName} (.+)$`, 'm'));
  assert.ok(m, `gate-rust.sh 里找不到 "run_gate ${gateName} ..." 这一行`);
  return m[1].trim();
}

/** 把 `env VAR="val" cargo ...` 拆成 { envVar, envVal, command }；没有 env 前缀则 envVar 为 null。 */
function parseShellGateCommand(line) {
  const m = line.match(/^env\s+([A-Z_]+)="([^"]*)"\s+(.+)$/);
  if (m) return { envVar: m[1], envVal: m[2], command: m[3].trim() };
  return { envVar: null, envVal: null, command: line.trim() };
}

/**
 * 抠出 gate-rust.sh 里 `run_gate <name> bash -c '` 到**收尾那一行单引号**之间的整块（已剥注释）。
 * 与 `extractShellGateLine` 的分工：那个抠单行门（五条可逐字对拍的），这个抠多行块。
 */
function extractShellGateBlock(strippedShell, gateName) {
  const lines = strippedShell.split('\n');
  const startIdx = lines.findIndex((l) => l.trim().startsWith(`run_gate ${gateName} bash -c '`));
  assert.ok(startIdx !== -1, `gate-rust.sh 里找不到 "run_gate ${gateName} bash -c '…" 这一块`);
  const endIdx = lines.findIndex((l, i) => i > startIdx && l.trim() === "'");
  assert.ok(endIdx !== -1, `gate-rust.sh 里 "run_gate ${gateName}" 那块没有收尾的单引号行`);
  return lines.slice(startIdx, endIdx + 1).join('\n');
}

/**
 * 从一段文本里抠出每一条 `cargo check …` 的**参数序列**。
 *
 * 只归一化两样、且只归一化这两样：
 *   · 命令前缀（`PATH="…" `）—— 两侧「怎么找到 NDK」的写法本来就不同（CI 用 job 级 env
 *     `$ANDROID_NDK_BIN`，本机现算），那不是「量什么」；
 *   · `>` 重定向与它的目标 —— CI 写 `$RUNNER_TEMP`，本机是 `mktemp -d` 的产物。
 * 剩下的全部逐字进比对：`--workspace` / `--target` / `--message-format` / 有没有 `--all-targets`。
 */
function cargoCheckArgs(text) {
  return text
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l.includes('cargo check '))
    .map((l) => l.slice(l.indexOf('cargo check ')).replace(/\s*>.*$/, '').trim());
}

/** 抠出 `node scripts/check-android-only-face.mjs …` 那一行，路径参数归一成 basename 再比。 */
function faceCheckerCall(text) {
  const m = text.match(/^\s*node (scripts\/check-android-only-face\.mjs.*)$/m);
  assert.ok(m, `找不到 check-android-only-face.mjs 的调用行：\n---\n${text}\n---`);
  return m[1].replace(/"\$\{?[A-Za-z_]\w*\}?\/([\w.-]+)"/g, '$1').trim();
}

// 6 个 CI 步骤（两个 fmt 共用同一本机门）：ci.yml 的 step 名 → gate-rust.sh 的 run_gate 名 → 若该步骤有 env 限定则给出变量名。
const GATES = [
  { ci: 'Check workspace formatting before native compilation', sh: 'fmt', envVar: null },
  { ci: 'Check formatting', sh: 'fmt', envVar: null },
  { ci: 'Clippy (deny warnings)', sh: 'clippy', envVar: 'RUSTFLAGS' },
  { ci: 'Rustdoc documentation invariants (deny four lints)', sh: 'rustdoc', envVar: 'RUSTDOCFLAGS' },
  { ci: 'Build', sh: 'build', envVar: null },
  { ci: 'Test', sh: 'test', envVar: null },
];

function loadStripped() {
  const ciRaw = readFileSync(new URL('../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const shRaw = readFileSync(join(ROOT, 'scripts/gate-rust.sh'), 'utf8');
  const controlledRaw = readFileSync(join(ROOT, 'scripts/run-native-dns-controlled-tests.mjs'), 'utf8');
  return { ci: stripComments(ciRaw), sh: stripComments(shRaw), controlled: controlledRaw };
}

/** ci.yml 里那条 Android 影响面完备性步骤的名字（本文件多处引用，抠成常量免得写歪一处不自知）。 */
const ANDROID_FACE_STEP = 'Android impact face must be registered (dep-info 对差)';

/**
 * 在 `gate-rust.sh` 里镜像了、但**不做逐字对拍**的 cargo 步骤。
 *
 * 三条都挂在脚本的 `--with-cross` 下（默认关闭，因为首跑要 `rustup target add` 联网）。
 * 不逐字对拍是因为它们都是多行 shell 块（rustup + jq + 循环 / 两条 cargo + 一条 node），
 * 不是一条 `run:` 命令 —— 上面那套 `extractRunCommand` 抠不出可比的单行。
 *
 * 🔴 **进这张名单的前提是「本机真的跑了这件事」**，不是「本机不跑但登记一下」。
 * 只登记名字不补跑法的话，本机对那件事的检出力恒为 0，而 `MIRRORED`（已镜像）这个名字
 * 会说谎 —— 完备性那条测试也就从「本机镜像跟得上 CI」退化成「名单跟得上 CI」。
 *
 * 且**不能因为不逐字对拍就不对拍**：漏掉的话 ci.yml 加第三个目标三元组 / 改一侧 cargo 的
 * 选择器而脚本没跟上，两边就分裂了。故下面两条测试各自钉住这几条门最容易漂的实质：
 *   · 跨目标那两条 → 目标三元组集合两侧一致；
 *   · dep-info 对差那条 → 两条 cargo check 的选择器两侧一致且互相对称、checker 调用形态一致。
 */
// The final two entries are deliberately CI-only manual candidate checks: the
// local no-kernel boundary prevents mirroring them in gate-rust.sh. They may not
// replace any ordinary gate. assertCandidateOnlySteps pins their exact opt-in
// scope and commands below; native-dns-ci-routing.test.mjs checks admission,
// payload identity, exact Rust namespaces and failure/evidence routing.
const CI_ONLY_SOURCE_STEP = 'Test additive RuntimeConfig client against loopback mocks';
const CI_ONLY_LOOPBACK_STEP = 'Run exact real App to paired kernel HOT loopback tests';
const CI_ONLY_SUBSCRIPTION_STEP = 'Verify native subscription JSON URI and UA preservation';
const CI_ONLY_BOOTSTRAP_STEP = 'Verify native helper bootstrap descriptor primitives';
const CI_ONLY_COMPILE_STEP = 'Compile native consumer test binaries without execution';
// This gate needs actual Windows producers and their compiled pin. Local
// gate-rust.sh does not mirror that prerequisite chain; never label it MIRRORED.
const CI_ONLY_WINDOWS_INSTALLER_STEP = 'Windows installer cleanup fixtures (native Rust producer)';
const CANDIDATE_BUILD_STEP = 'Build actual candidate helper bytes and freeze the separate App identity';
const CANDIDATE_RUNTIME_STEP = 'Run exact controlled App lifecycle hot-switch and stats tests';
const MIRRORED_BUT_NOT_VERBATIM = [
  'Cross-check platform targets (cfg(windows) / cfg(macos) / cfg(android) / cfg(ios) 分支)',
  'Cross-target exemptions must still be necessary',
  ANDROID_FACE_STEP,
  CANDIDATE_BUILD_STEP,
  CANDIDATE_RUNTIME_STEP,
];

function assertCandidateOnlySteps(ci, controlled) {
  const build = extractCiStepBlock(ci, CANDIDATE_BUILD_STEP);
  const runtime = extractCiStepBlock(ci, CANDIDATE_RUNTIME_STEP);
  for (const block of [build, runtime]) {
    const condition = block.match(/\sif: ([^\n]+)/)?.[1];
    assert.ok(condition?.includes("needs.cheap_format.outputs.native_dns_candidate != ''"));
    assert.match(block, /set -euo pipefail/);
    assert.doesNotMatch(block, /continue-on-error/);
  }
  const commands = (block) => block.split('\n').map((line) => line.trim()).filter((line) => line.startsWith('cargo '));
  assert.deepEqual(commands(build), [
    'cargo build --locked -p polaris-helper --bin polaris-helper 2>&1 | tee native-dns-ci-evidence/helper-build.log',
    'cargo build --locked -p polaris-windows-cleaner --bin polaris-cleaner 2>&1 | tee native-dns-ci-evidence/cleaner-build.log',
  ]);
  assert.match(build, /if \[ "\$RUNNER_OS" = Windows \]; then/);
  assert.ok(build.includes('export POLARIS_BUILD_ID="$CORE_CANDIDATE"'));
  assert.ok(build.indexOf('export POLARIS_BUILD_ID=') < build.indexOf('cargo build --locked'));
  assert.ok(build.indexOf('cargo build --locked') < build.indexOf('node scripts/prepare-core-identity.mjs'));
  assert.deepEqual(commands(runtime), []);
  assert.ok(runtime.includes('node scripts/run-native-dns-controlled-tests.mjs "${tests[@]}"'));
  assert.match(runtime, /node --test scripts\/native-dns-ci-routing\.test\.mjs/);
  assert.match(runtime, /scripts\/run-native-dns-controlled-tests\.test\.mjs/);
  assert.ok(controlled.includes("args: ['test', '--locked', '-p', 'polaris', '--lib', '--no-run', '--message-format=json']"));
  assert.equal((controlled.match(/'--no-run'/g) ?? []).length, 2);
  assert.ok(controlled.includes("args: [name, '--exact', '--ignored', '--nocapture', '--test-threads=1']"));
  assert.match(controlled, /summaries\.length === 1/);
  assert.match(controlled, /test result: ok\. 1 passed; 0 failed; 0 ignored;/);
  assert.match(runtime, /!cancelled\(\)/);
  assert.match(runtime, /steps\.workspace_build\.outcome == 'success'/);
}

test('the two candidate-only Cargo steps retain exact commands, admitted scope and failure checks', () => {
  const { ci, controlled } = loadStripped();
  assertCandidateOnlySteps(ci, controlled);
});
for (const [name, source, before, after] of [
  ['candidate admission removed', 'ci', "needs.cheap_format.outputs.native_dns_candidate != ''", 'true'],
  ['helper build selector weakened', 'ci', 'cargo build --locked -p polaris-helper --bin polaris-helper', 'cargo build -p polaris-helper'],
  ['cleaner selector changed', 'ci', 'cargo build --locked -p polaris-windows-cleaner --bin polaris-cleaner', 'cargo build --locked -p polaris-helper --bin polaris-helper'],
  ['controlled runner disconnected', 'ci', 'node scripts/run-native-dns-controlled-tests.mjs', 'node scripts/other.mjs'],
  ['controlled build selector weakened', 'controlled', "'--lib', '--no-run'", "'--no-run'"],
  ['ignored tests broadened', 'controlled', "name, '--exact', '--ignored'", "name, '--ignored'"],
  ['zero-test evidence accepted', 'controlled', 'test result: ok. 1 passed; 0 failed; 0 ignored;', 'test result: ok.'],
  ['duplicated verdict accepted', 'controlled', 'summaries.length === 1', 'summaries.length >= 1'],
]) test(`candidate-only Cargo registration rejects ${name}`, () => {
  const fixture = loadStripped();
  assert.ok(fixture[source].includes(before));
  fixture[source] = fixture[source].replaceAll(before, after);
  assert.throws(() => assertCandidateOnlySteps(fixture.ci, fixture.controlled), { name: 'AssertionError' });
});

/**
 * 从一段文本里抠出 rust 目标三元组（`x86_64-pc-windows-msvc` / `aarch64-linux-android` 这类）。
 *
 * 🔴 **`linux` 这一支是 2026-09-04 补的，补之前本函数对 android 三元组结构性失明**：
 * 旧模式只认 `pc|apple|unknown` 三种 vendor 段，而 `aarch64-linux-android` 的 vendor 段被省略
 * （arch-os-env 三段形态）⇒ 它一个字符都匹配不到。后果不是「少抓一条」而是**这条对拍门对本次改动
 * 恰好完全失明**：ci.yml 加了 android 目标、gate-rust.sh 没跟上，两侧集合都算作「没有 android」
 * 而判等 —— 门在但没牙，且哑在它最该说话的那一次。
 *
 * 负向前瞻 `(?<![-\w])` 是随之必需的：加了 `linux` 之后，`x86_64-unknown-linux-gnu` 里的
 * `unknown-linux-gnu` 会被当成第二个「三元组」抠出来，凭空造出一个两侧对不上的幽灵条目。
 */
function targetTriples(text) {
  return [...new Set(text.match(/(?<![-\w])\w+-(?:pc|apple|unknown|linux)-[\w-]+/g) ?? [])].sort();
}

/**
 * ci.yml 与 gate-rust.sh 跨目标门覆盖的**目标集合**必须一致。
 *
 * 这两条门不逐字对拍（多行 shell 块），但目标清单是它们最实质、也最容易只改一边的部分：
 * CI 加一个 `aarch64-apple-darwin` 而脚本还只跑两个，本机就永远验不到那个平台的
 * `cfg` 分支，而「本机跑过了」会被当成已验。
 */
test('跨目标门覆盖的目标三元组，ci.yml 与 gate-rust.sh 一致', () => {
  const { ci, sh } = loadStripped();
  const ciBlock = extractCiStepBlock(ci, MIRRORED_BUT_NOT_VERBATIM[0]);
  const ciTriples = targetTriples(ciBlock);
  const shTriples = targetTriples(sh);

  assert.ok(ciTriples.length > 0, 'ci.yml 的跨目标门里一个三元组都没抠到 —— 取材面是空的，本断言恒真');
  assert.deepEqual(
    shTriples,
    ciTriples,
    `跨目标门的目标集合不一致：\n  ci.yml      = ${JSON.stringify(ciTriples)}\n` +
      `  gate-rust.sh = ${JSON.stringify(shTriples)}\n` +
      '两边都要改：scripts/gate-rust.sh 的 cross-clippy 门与 .github/workflows/ci.yml 的跨目标步骤。'
  );

  // 豁免表是两条门共同的输入，任一侧改了路径/文件名，另一侧会读到空表而静默放行。
  for (const [name, text] of [['ci.yml', ci], ['gate-rust.sh', sh]]) {
    assert.ok(
      text.includes('scripts/cross-target-exempt.json'),
      `${name} 里找不到 scripts/cross-target-exempt.json —— 豁免表的路径两侧必须同源`
    );
  }
});

/**
 * 「随仓路径源零诊断」门：两侧调的是**同一条命令**，且它的目标清单就是跨目标门的那一份。
 *
 * 这一步在 ci.yml 里不直接出现 `cargo`（cargo 在被调脚本里），所以下面那条按「跑 cargo 的步骤」
 * 取材的完备性断言看不见它 —— 不在这里单独钉，CI 侧删掉这一步或本机侧漏掉镜像都不会有任何东西变红。
 * 目标清单写在两侧的调用行里而不在脚本里：钉成与跨目标门同一份，新增一个交叉目标时这一步不会落下。
 */
const VENDORED_STEP = 'Vendored path sources stay diagnostic-free';

test(`「${VENDORED_STEP}」门：ci.yml 与 gate-rust.sh 逐字调同一条命令，目标清单同跨目标门`, () => {
  const { ci, sh } = loadStripped();
  const ciCommand = extractRunCommand(extractCiStepBlock(ci, VENDORED_STEP), `ci.yml "${VENDORED_STEP}"`);
  const shLine = sh.split('\n').map((l) => l.trim()).find((l) => l.startsWith('run_gate cross-vendored '));
  assert.ok(shLine, 'gate-rust.sh 里找不到 "run_gate cross-vendored …" 这一行');
  assert.equal(shLine.slice('run_gate cross-vendored '.length), ciCommand, '命令本身不一致（ci.yml → gate-rust.sh）');

  assert.ok(
    ciCommand.startsWith('bash scripts/check-vendored-path-sources.sh '),
    `这一步调的不是那份脚本：${ciCommand}`
  );
  const crossTriples = targetTriples(extractCiStepBlock(ci, MIRRORED_BUT_NOT_VERBATIM[0]));
  assert.ok(crossTriples.length > 0, '跨目标门里一个三元组都没抠到 —— 下面的比对恒真');
  assert.deepEqual(
    targetTriples(ciCommand),
    crossTriples,
    '随仓路径源门的目标清单与跨目标门不是同一份 —— 新增的交叉目标在这一步被落下了'
  );

  // 脚本自己不许再带一份清单，也必须真的在数诊断（而不是只看 cargo 的退出码）。
  const script = stripComments(readFileSync(join(ROOT, 'scripts/check-vendored-path-sources.sh'), 'utf8'));
  assert.deepEqual(targetTriples(script.replace(/aarch64-linux-android\b/g, '')), [], '脚本里出现了写死的目标三元组');
  assert.ok(script.includes('--message-format=json >'), '脚本没有把 cargo 的 JSON 诊断流落盘');
  assert.ok(script.includes('.reason == "compiler-message" and .package_id == $id'), '脚本没有按包 id 数诊断');
});

/**
 * 「Android 影响面 dep-info 对差」门的**实质**两侧一致。
 *
 * 这条门抓的是：`ANDROID_IMPACT_SCOPES` 是 android.yml 的唯一触发面，新增 Android 专属源文件
 * 而不登记 ⇒ APK 腿永远不为它跑。判据 = android 编译面 − host 编译面，两条 `cargo check` 的
 * **选择器**就是这个差集的取材面本身 —— 改一侧而另一侧没跟上，差集当场失真。
 */
test(`「${ANDROID_FACE_STEP}」门的实质，ci.yml 与 gate-rust.sh 一致`, () => {
  const { ci, sh } = loadStripped();
  const ciBlock = extractCiStepBlock(ci, ANDROID_FACE_STEP);
  const shBlock = extractShellGateBlock(sh, 'android-face');

  const ciCargo = cargoCheckArgs(ciBlock);
  const shCargo = cargoCheckArgs(shBlock);
  // 取材面自检：两条（android 面 + host 面）。抠到 0 条或 1 条时下面的 deepEqual 可能恒真。
  assert.equal(ciCargo.length, 2, `ci.yml 该步应抠到 2 条 cargo check，实为 ${ciCargo.length} 条：${JSON.stringify(ciCargo)}`);
  assert.deepEqual(
    shCargo,
    ciCargo,
    `两条 cargo check 的选择器不一致（ci.yml → gate-rust.sh）：\n  ci.yml       = ${JSON.stringify(ciCargo)}\n` +
      `  gate-rust.sh = ${JSON.stringify(shCargo)}\n` +
      '两边都要改：scripts/gate-rust.sh 的 android-face 门与 .github/workflows/ci.yml 的同名步骤。'
  );

  // 🔴 两侧选择器必须**逐字对称**：android 那条只比 host 那条多一个 `--target`。
  //    成因见 ci.yml 同名步的 🔴 段 —— host 侧一旦更宽（例如借用已有的 clippy --all-targets 省钱），
  //    `#[cfg(any(target_os = "android", test))]` 的文件会同时落进 host 面而从差集里掉出去，
  //    方向是 **fail-open**：门说全绿而实际漏了。这个 pattern 仓里真的在用
  //    （`src-tauri/src/runtime/stats/source.rs` 的三个通道常量）。
  const [androidCmd, hostCmd] = ciCargo;
  assert.equal(
    androidCmd.replace(' --target aarch64-linux-android', ''),
    hostCmd,
    `两侧选择器不对称（android 那条应当只比 host 那条多一个 --target）：\n  android = ${androidCmd}\n  host    = ${hostCmd}`
  );

  // `--all-targets` 两侧都不许有：android 侧带它会让 polaris-helper 撞 4 条 E0433（同上注释）。
  // 上面那条对称性拦得住「只有一侧带」，拦不住「两侧一起带」，故这里单独钉一句。
  for (const cmd of ciCargo) {
    assert.ok(
      !cmd.includes('--all-targets'),
      `dep-info 对差的两侧都不能带 --all-targets（android 侧会撞 E0433）：${cmd}`
    );
  }

  assert.equal(
    faceCheckerCall(shBlock),
    faceCheckerCall(ciBlock),
    'checker 的调用形态不一致（ci.yml → gate-rust.sh）：两侧都要 --android / --host 各喂一份，' +
      '少一个 flag 或换个脚本名都会让本机量的不是 CI 量的那件事。'
  );
});

// ── 切片自检：证明 android-face 那块抠的是块内的命令，不是块外注释里的同名字样 ──
test('切片自检：android-face 块的抠取不吃块外/注释里的同名 cargo check', () => {
  const synthetic = [
    '  # 历史写法：cargo check --workspace --all-targets --target aarch64-linux-android（已废弃）',
    "  run_gate android-face bash -c '",
    '    set -euo pipefail',
    '    cargo check --workspace --message-format=json-render-diagnostics > "$OUT/host-units.json"  # 就是这条',
    "  '",
    '  # 收尾之后又提一句 cargo check --workspace --all-targets，不该被抠进来',
  ].join('\n');

  const block = extractShellGateBlock(stripComments(synthetic), 'android-face');
  assert.deepEqual(
    cargoCheckArgs(block),
    ['cargo check --workspace --message-format=json-render-diagnostics'],
    '抠到的不是块内那条真命令 —— 切片被块外注释或行内注释污染了'
  );
});

/**
 * **完备性**：逐条对拍只能证明「列出来的这几条一致」，证明不了「ci.yml 没有别的 cargo 门」。
 * 没有这条断言，CI 新增一个 Rust 门时本机脚本会静默落后 —— 覆盖面就由夹具（GATES 这张写死的表）
 * 决定，而不是由判据（ci.yml 里实际有哪些 cargo 门）决定。本仓踩过这个形状，故钉死。
 */
function assertCargoStepCoverage(ci) {
  const lines = ci.split('\n');
  const cargoSteps = [];
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(/^\s{6}- name: (.+)$/);
    if (!m) continue;
    const block = extractCiStepBlock(ci, m[1]);
    const body = block.replace(/^\s*- name:.*$/m, '');
    // 判据写成「**除了**已知的非门形态，一律算门」，而不是「只认白名单里的子命令」。
    //
    // 白名单写法（`['fmt','clippy','test','doc','build','check'].includes(sub)`）实测有洞：
    // 往 ci.yml 插一条 `cargo deny check` 本条断言不红 —— 因为 `deny` 不在表里。那等于
    // 「只认识我想得到的门」，CI 加一条没想到的（deny / audit / nextest / udeps…）就静默漏掉，
    // 与本断言要修的毛病同形。故反过来：默认算门，只排除明确不是门的那几种形态。
    const invocations = [...body.matchAll(/\bcargo\s+(\+?[\w-]+)([^\n]*)/g)];
    const isGate = invocations.some(([, sub, rest]) => {
      // `cargo <x> --version` / `cargo --version`：装完工具链自检版本，不是门。
      if (/(^|\s)--version(\s|$)/.test(rest)) return false;
      // `cargo install …`：装工具，不是门。
      if (sub === 'install') return false;
      return true;
    });
    // The controlled step delegates its single Cargo build to this exact runner;
    // assertCandidateOnlySteps above pins both the call and the runner's selectors.
    const delegatedGate = /\bnode scripts\/run-native-dns-controlled-tests\.mjs(?:\s|$)/.test(body)
      || /\bnode scripts\/runtime-config-loopback\.mjs run\b/.test(body);
    if (isGate || delegatedGate) cargoSteps.push(m[1]);
  }
  assert.ok(cargoSteps.length > 0, '一个跑 cargo 的步骤都没抠到 —— 取材面是空的，本断言恒真');

  const covered = new Set([...GATES.map((g) => g.ci), ...MIRRORED_BUT_NOT_VERBATIM, CI_ONLY_COMPILE_STEP, CI_ONLY_SOURCE_STEP, CI_ONLY_LOOPBACK_STEP, CI_ONLY_BOOTSTRAP_STEP, CI_ONLY_SUBSCRIPTION_STEP, CI_ONLY_WINDOWS_INSTALLER_STEP]);
  const orphans = cargoSteps.filter((n) => !covered.has(n));
  assert.deepEqual(
    orphans,
    [],
    `ci.yml 有跑 cargo 的步骤既不在 GATES 也不在非逐字镜像名单里：${JSON.stringify(orphans)}\n` +
      '要么把它加进 scripts/gate-rust.sh 与 GATES（可逐字对拍的单行命令），' +
      '要么（多行 shell 块，抠不出单行）加进 MIRRORED_BUT_NOT_VERBATIM 并为它补一条针对性对拍。'
  );

  const stale = [...MIRRORED_BUT_NOT_VERBATIM, CI_ONLY_COMPILE_STEP, CI_ONLY_SOURCE_STEP, CI_ONLY_LOOPBACK_STEP, CI_ONLY_BOOTSTRAP_STEP, CI_ONLY_SUBSCRIPTION_STEP, CI_ONLY_WINDOWS_INSTALLER_STEP].filter((n) => !cargoSteps.includes(n));
  assert.deepEqual(stale, [], `非逐字镜像名单里有 ci.yml 已经没有的步骤（名单陈旧）：${JSON.stringify(stale)}`);
}

test('ci.yml 里跑 cargo 的步骤，要么在 GATES 里、要么在非逐字镜像名单里', () => {
  const { ci } = loadStripped();
  assertCargoStepCoverage(ci);
  for (const command of ['cargo deny check', 'node scripts/runtime-config-loopback.mjs run', 'node scripts/run-native-dns-controlled-tests.mjs --source-contracts']) {
    const added = ci + `\n  orphan_fixture:\n    steps:\n      - name: Unregistered native gate\n        run: ${command}\n`;
    assert.throws(() => assertCargoStepCoverage(added), /Unregistered native gate/);
  }
  assert.throws(() => assertCargoStepCoverage(ci.replace('run: node scripts/runtime-config-loopback.mjs run', 'run: echo omitted')), /名单陈旧/);
  assert.throws(() => assertCargoStepCoverage(ci.replace(extractCiStepBlock(ci, CI_ONLY_WINDOWS_INSTALLER_STEP), '')), /名单陈旧/);
});

for (const gate of GATES) {
  test(`gate-rust.sh 的 "${gate.sh}" 门与 ci.yml 步骤 "${gate.ci}" 逐字一致`, () => {
    const { ci, sh } = loadStripped();
    const ciBlock = extractCiStepBlock(ci, gate.ci);
    const ciCommand = extractRunCommand(ciBlock, `ci.yml "${gate.ci}"`);

    const shLine = extractShellGateLine(sh, gate.sh);
    const { envVar, envVal, command: shCommand } = parseShellGateCommand(shLine);

    assert.equal(shCommand, ciCommand, `命令本身不一致（ci.yml → gate-rust.sh）`);

    if (gate.envVar) {
      const ciEnvVal = extractEnvVar(ciBlock, gate.envVar);
      assert.ok(ciEnvVal !== null, `ci.yml "${gate.ci}" 步骤应有 env.${gate.envVar}，没抠到`);
      assert.equal(envVar, gate.envVar, `gate-rust.sh "${gate.sh}" 门应带 env ${gate.envVar}`);
      assert.equal(envVal, ciEnvVal, `${gate.envVar} 取值不一致（ci.yml → gate-rust.sh）`);
    } else {
      assert.equal(envVar, null, `gate-rust.sh "${gate.sh}" 门不该带 env 前缀，ci.yml 该步骤没有 env`);
    }
  });
}

// ── 切片自检：证明取到的是命令行本身，不是注释里的同名字样 ──
// 构造一段合成文本：真正的 run: 命令之外，前后各埋一条「提到同一个 cargo 子命令但参数不同」的
// 注释诱饵。stripComments + extractRunCommand 若被诱饵污染，抠出来的会是诱饵那句而非真命令，
// 下面的 assert.equal 会直接报出抠到的字符串——红了就能看见污染的是哪一句。
test('切片自检：stripComments 不会把注释里的同名 cargo 命令当成真命令抠出来', () => {
  const decoyBefore = '      # 参考命令：cargo fmt --all -- --write（历史遗留写法，已废弃）';
  const trailingDecoy = '      - name: Check formatting  # 注意别写成 cargo fmt --check（无 --all）';
  const synthetic = [
    decoyBefore,
    trailingDecoy,
    '        run: cargo fmt --all -- --check  # 就是这条，真的',
    '      - name: Build',
  ].join('\n');

  const stripped = stripComments(synthetic);
  // 两条诱饵必须已被剥空/剥断——先证明剥注释本身生效。
  assert.equal(stripped.split('\n')[0], '', '整行注释诱饵没被剥空');
  assert.ok(!stripped.includes('无 --all'), '行内注释诱饵没被剥掉');

  const block = extractCiStepBlock(stripped, 'Check formatting');
  const command = extractRunCommand(block, 'synthetic');
  assert.equal(command, 'cargo fmt --all -- --check', '抠到的不是真命令，切片被注释诱饵污染了');
});

test('切片自检：行内 # 诱饵不会截断真实命令参数', () => {
  // 若行内注释剥除逻辑失手把「井号出现在参数里」误判成注释起点，会把 --check 之后的内容切掉。
  // 这里真命令本身不含 #，但同一整段落里紧邻一条含 # 的散文，确认不会跨行污染到 run: 那一行。
  const synthetic = [
    '      - name: Check formatting',
    '        # 这行提到 clippy -D warnings 但那是别的门，不是这一个',
    '        run: cargo fmt --all -- --check',
  ].join('\n');
  const stripped = stripComments(synthetic);
  const block = extractCiStepBlock(stripped, 'Check formatting');
  const command = extractRunCommand(block, 'synthetic');
  assert.equal(command, 'cargo fmt --all -- --check');
});

// This explicit manual compiler check does not replace or broaden the local gate.
function assertCompileOnlyStep(ci) {
  const block = extractCiStepBlock(ci, CI_ONLY_COMPILE_STEP);
  assert.equal(extractRunCommand(block, CI_ONLY_COMPILE_STEP), 'cargo test --locked --workspace --no-run');
  assert.deepEqual([...block.matchAll(/^        if: (.+)$/gm)].map((m) => m[1]),
    ["needs.cheap_format.outputs.app_native_compile_only == 'true'"]);
  assert.doesNotMatch(block, /continue-on-error|--ignored|--release|\|\| true/);
  assert.match(ci, /app_native_compile_candidate:\n        description:.*\n        type: string\n        default: ''/);
  const admission = extractCiStepBlock(ci, 'Admit exact App native compile-only request');
  assert.match(admission, /github\.event_name == 'workflow_dispatch'/);
  assert.match(admission, /candidate !== e\.GITHUB_SHA/);
  assert.match(admission, /selected\.os !== 'ubuntu-22\.04'/);
  assert.match(admission, /compile_only=true/);
}
test('manual native compilation compiles test binaries without executing or replacing ordinary gates', () => {
  const { ci } = loadStripped();
  assertCompileOnlyStep(ci);
  for (const [before, after] of [
    ['cargo test --locked --workspace --no-run', 'cargo test --locked --workspace'],
    ["if: needs.cheap_format.outputs.app_native_compile_only == 'true'", 'if: true'],
    ["selected.os !== 'ubuntu-22.04'", 'false'],
  ]) assert.throws(() => assertCompileOnlyStep(ci.replaceAll(before, after)));
});

// Existing RuntimeConfig source tests are independent CI-only checks, not a local
// Rust-gate replacement; their complete isolation is checked by the routing suite.
test('existing RuntimeConfig CI-only command has an explicit narrow registration', () => {
  const { ci } = loadStripped();
  const block = extractCiStepBlock(ci, CI_ONLY_SOURCE_STEP);
  assert.equal(extractRunCommand(block, CI_ONLY_SOURCE_STEP), 'cargo test --locked -p polaris-singbox-grpc --test runtime_config');
  const source = extractCiJobBlock(ci, 'runtime_config_source_tests');
  assert.ok(source.includes(block));
  assert.doesNotMatch(source, /fetch-core|prepare-core-identity|sudo|--ignored|--release|continue-on-error/);
  assert.match(source, /This job is not full CI, native or HOT acceptance/);
});


function extractCiJobBlock(ci, name) {
  const tail = ci.split(`\n  ${name}:\n`)[1];
  assert.ok(tail, `Missing CI job ${name}`);
  return tail.split(/\n {2}[a-z][a-z0-9_-]*:\n/)[0];
}

function assertLoopbackOnlyStep(ci, sh, runner) {
  const job = extractCiJobBlock(ci, 'runtime_config_loopback');
  const block = extractCiStepBlock(job, CI_ONLY_LOOPBACK_STEP);
  assert.equal(extractRunCommand(block, CI_ONLY_LOOPBACK_STEP), 'node scripts/runtime-config-loopback.mjs run');
  assert.match(block, /RUNTIME_CONFIG_LOOPBACK_INPUTS: \$\{\{ toJSON\(inputs\) \}\}/);
  assert.match(job, /^    needs: cheap_format$/m);
  assert.match(job, /^    if: needs\.cheap_format\.outputs\.runtime_config_loopback_only == 'true'$/m);
  assert.doesNotMatch(job, /continue-on-error|fetch-core|prepare-core-identity|Swatinem\/rust-cache|\|\| true/);
  assert.doesNotMatch(sh, /runtime-config-loopback|real_hot_dispatcher|real_hot_public_rpc/);
  assert.match(runner, /import \{ exactPass, runtimeEnvironment, selectExecutable \} from '\.\/run-native-dns-controlled-tests\.mjs'/);
  const cargoCalls = [...runner.matchAll(/(?:capture|execute)\([^;\n]*?'cargo', (\[[^\]]+\])/g)].map((match) => match[1]);
  assert.deepEqual(cargoCalls, [
    "['metadata', '--locked', '--no-deps', '--format-version=1']",
    "['test', '--locked', '-p', 'polaris', '--lib', '--no-run', '--message-format=json']",
  ]);
  const names = [...runner.matchAll(/'(runtime::proxy::tests::runtime_config_real_core::[a-z_]+)'/g)].map((match) => match[1]);
  assert.deepEqual(names, [
    'runtime::proxy::tests::runtime_config_real_core::real_hot_dispatcher_apply_and_status',
    'runtime::proxy::tests::runtime_config_real_core::real_hot_dispatcher_persistence_retry',
    'runtime::proxy::tests::runtime_config_real_core::real_hot_public_rpc_contract',
    'runtime::proxy::tests::runtime_config_real_core::real_hot_dispatcher_rejections_preserve_runtime',
  ]);
  assert.deepEqual(LOOPBACK_TESTS, names);
  assert.match(runner, /for \(const name of TESTS\)/);
  assert.match(runner, /program: selected\.executable,/);
  assert.match(runner, /args: \[name, '--exact', '--ignored', '--nocapture', '--test-threads=1'\]/);
  assert.match(runner, /selectExecutable\(messages, metadata, ROOT\)/);
  assert.match(runner, /demand\(exactPass\(observed\),/);
  assert.match(runner, /demand\(result\.tests\.length === 4,/);
  assert.doesNotMatch(runner, /cargo test[^\n]*--ignored|--test-threads=0|--include-ignored/);
}

test('real HOT delegated runner is explicitly CI-only with exact compile and four libtest selectors', () => {
  const { ci, sh } = loadStripped();
  const runner = readFileSync(join(ROOT, 'scripts/runtime-config-loopback.mjs'), 'utf8');
  assertLoopbackOnlyStep(ci, sh, runner);
  for (const [before, after] of [
    ["'--lib', '--no-run'", "'--lib'"],
    ["'-p', 'polaris', '--lib'", "'--workspace'"],
    ["args: [name, '--exact', '--ignored'", "args: [name, '--ignored'"],
    ['demand(exactPass(observed),', 'demand(true,'],
    ['demand(result.tests.length === 4,', 'demand(result.tests.length >= 0,'],
    ['real_hot_public_rpc_contract', 'other_ignored_test'],
  ]) {
    assert.ok(runner.includes(before), before);
    assert.throws(() => assertLoopbackOnlyStep(ci, sh, runner.replaceAll(before, after)), before);
  }
  for (const [before, after] of [
    ["if: needs.cheap_format.outputs.runtime_config_loopback_only == 'true'", 'if: true'],
    ['run: node scripts/runtime-config-loopback.mjs run', 'run: node scripts/runtime-config-loopback.mjs run --all'],
  ]) assert.throws(() => assertLoopbackOnlyStep(ci.replace(before, after), sh, runner));
});

test('CI step and source-job scanners stop at sibling jobs and unnamed steps', () => {
  const fixture = '  first:\n    steps:\n      - name: Exact\n        run: cargo check --locked\n'
    + '      - uses: actions/checkout@v7\n        with:\n          persist-credentials: false\n'
    + '  second:\n    steps:\n      - name: Other\n        run: cargo deny check\n';
  assert.equal(extractRunCommand(extractCiStepBlock(fixture, 'Exact'), 'Exact'), 'cargo check --locked');
  assert.doesNotMatch(extractCiStepBlock(fixture, 'Exact'), /checkout|second|cargo deny/);
  const last = fixture.replace('      - uses: actions/checkout@v7\n        with:\n          persist-credentials: false\n', '');
  assert.doesNotMatch(extractCiStepBlock(last, 'Exact'), /second|cargo deny/);
  const { ci } = loadStripped();
  const added = ci.replace('\n  windows_launcher_diagnostics:\n', '\n  sibling_fixture:\n    steps:\n      - run: sudo cargo test --ignored\n\n  windows_launcher_diagnostics:\n');
  assert.doesNotMatch(extractCiJobBlock(added, 'runtime_config_source_tests'), /sudo|--ignored|sibling_fixture/);
});


test('formal HOT adds only the exact helper build and existing identity preparation before App compile', () => {
  const source = readFileSync(join(ROOT, 'scripts/runtime-config-loopback-formal.mjs'), 'utf8');
  assert.match(source, /\['build', '--locked', '-p', 'polaris-helper', '--bin', 'polaris-helper', '--message-format=json'\]/);
  assert.match(source, /selectFormalHelper\(messages, metadata, root, target\)/);
  assert.match(source, /scripts\/prepare-core-identity\.mjs/);
  assert.doesNotMatch(source, /--ignored|--include-ignored|cargo test|program:.*polaris-helper/);
  const runner = readFileSync(join(ROOT, 'scripts/runtime-config-loopback.mjs'), 'utf8');
  assert.ok(runner.indexOf('await prepareFormalCarrier') < runner.indexOf("execute('app-test-build'"));
});

function assertBootstrapPrimitives(ci) {
  const block = extractCiStepBlock(ci, CI_ONLY_BOOTSTRAP_STEP);
  assert.ok(block.includes("runner.os == 'macOS' || runner.os == 'Linux'"));
  assert.ok(block.includes("needs.cheap_format.outputs.app_native_compile_only != 'true'"));
  assert.ok(block.includes('!cancelled()'));
  assert.ok(block.includes('run: node scripts/run-native-dns-controlled-tests.mjs --bootstrap-contracts'));
  assert.deepEqual(BOOTSTRAP_TESTS, [
    'polaris-helper-client:manager::tests::bootstrap_identity::bootstrap_identity_render_is_platform_specific',
    'polaris-helper-client:manager::tests::bootstrap_identity::native_bootstrap_descriptor_identity',
  ]);
  assert.ok(!block.includes('sudo'));
}
test('native bootstrap primitives execute exact nonempty Linux and macOS tests without administrator operations', () => {
  const { ci } = loadStripped(); assertBootstrapPrimitives(ci);
  for (const original of ['--bootstrap-contracts', "runner.os == 'macOS'", '!cancelled()']) {
    const block = extractCiStepBlock(ci, CI_ONLY_BOOTSTRAP_STEP);
    assert.throws(() => assertBootstrapPrimitives(ci.replace(block, block.replace(original, 'omitted'))), { name: 'AssertionError' });
  }
});

const SUBSCRIPTION_NATIVE_TESTS = [
  "polaris-store:tests::store_tests::unsupported_vision_transport_remains_repairable_across_real_store",
  "polaris:runtime::speedtest::tests::vision_transport_temp_plan_reports_fixed_failure_without_losing_good_nodes",
  "polaris-config-engine:builder::outbound::tests::vision_transport_admission_preserves_supported_native_streams",
  "polaris-config-engine:builder::outbounds::tests::vision_invalid_nodes_preserve_explicit_targets_and_good_auto_candidates",
  "polaris-net-stack:share_link::tests::vision_transport_import_retains_original_nodes_and_safe_warnings",
  "polaris-net-stack:singbox_import::tests::vision_extra_transports_are_retained_with_repairable_warning",
  "polaris-net-stack:singbox_import::tests::naive_json_h2_h3_survive_dedup_serde_and_full_generation",
  "polaris-net-stack:singbox_import::tests::naive_json_quic_flag_is_not_an_unsupported_transport_object",
  "polaris-net-stack:share_link::tests::naive_quic_community_uri_preserves_h3_and_existing_tls_query",
  "polaris-net-stack:share_link::tests::naive_quic_multiline_subscription_keeps_h2_and_h3",
  "polaris-net-stack:subscription::tests::fetch_tests::generated_default_and_custom_ua_reach_real_fetch_init",
  "polaris:commands::subscription::tests::reconcile_tests::naive_json_refresh_reorder_preserves_h3_through_real_store",
  "polaris:commands::subscription::tests::ua_tests::effective_ua_default_and_persisted_override_matrix",
  "polaris:runtime::http::tests::subscription_default_and_override_ua_are_real_wire_headers",
  "polaris:runtime::config::tests::subscription_edit_publishes_owner_only_after_real_storage_success",
  "polaris:commands::subscription::tests::boundary_regressions::edit_supersession_drops_the_actual_awaited_pipeline_and_cancels_providers",
  "polaris:commands::subscription::tests::boundary_regressions::refresh_pipeline_obeys_existing_deadline_without_retrying_or_disabling_owner",
  "polaris:commands::subscription::tests::boundary_regressions::late_refresh_publication_cannot_invalidate_new_saved_edit_intent",
  "polaris:commands::subscription::tests::boundary_regressions::edit_refresh_uses_effective_request_identity_not_metadata_or_requested_ua_text",
  "polaris:runtime::subscription_refresh::tests::saved_edit_supersedes_old_work_without_old_drop_releasing_new_owner",
  "polaris:runtime::subscription_refresh::tests::repeated_edits_and_aba_cannot_publish_old_progress_or_commit",
  "polaris:runtime::subscription_scheduler::tests::superseded_refresh_is_not_a_scheduler_failure_or_announcement",
  "polaris-config-engine:user_config::app_config::tests::utls_default_initialization_and_missing_fields_preserve_explicit_policies",
  "polaris-config-engine:user_config::utls::tests::initial_missing_and_null_policy_follow_saved_fingerprint_in_real_generator",
  "polaris-config-engine:user_config::utls::tests::policy_projection_enters_refresh_and_missing_is_follow",
  "polaris-config-engine:user_config::utls::tests::node_intent_global_policy_protocol_matrix_keeps_stored_nodes_unchanged",
  "polaris-store:tests::store_tests::new_installation_uses_follow_and_preserves_explicit_saved_policies",
  "polaris:runtime::proxy::pending_changes::tests::global_utls_change_creates_runtime_debt_and_missing_restores_follow_projection"
];
function assertSubscriptionNative(ci) {
  const block = extractCiStepBlock(ci, CI_ONLY_SUBSCRIPTION_STEP);
  assert.ok(block.includes("runner.os == 'macOS' || runner.os == 'Linux'"));
  assert.ok(block.includes("needs.cheap_format.outputs.app_native_compile_only != 'true'"));
  assert.ok(block.includes('!cancelled()'));
  assert.ok(block.includes('run: node scripts/run-native-dns-controlled-tests.mjs --source-contracts'));
  assert.deepEqual(SOURCE_TESTS, SUBSCRIPTION_NATIVE_TESTS);
  assert.ok(!block.includes('sudo'));
}
test('subscription native gate preserves exact JSON URI edit ownership uTLS and actual wire selectors', () => {
  const { ci } = loadStripped(); assertSubscriptionNative(ci);
  for (const original of ['--source-contracts', "runner.os == 'macOS'", '!cancelled()']) {
    const block = extractCiStepBlock(ci, CI_ONLY_SUBSCRIPTION_STEP);
    assert.throws(() => assertSubscriptionNative(ci.replace(block, block.replace(original, 'omitted'))), { name: 'AssertionError' });
  }
});


function assertWindowsMaintenanceConsumer(ci) {
  const native = extractCiStepBlock(ci, CI_ONLY_WINDOWS_INSTALLER_STEP);
  assert.equal(native.match(/^\s*if: (.+)$/m)?.[1],
    "needs.cheap_format.outputs.app_native_compile_only != 'true' && (runner.os == 'Windows')");
  assert.match(native, /shell: bash/);
  assert.doesNotMatch(native, /continue-on-error|\|\| true|--ignored|--no-run|SIGNED_INERT|POLARIS_C05_POLICY_VALIDATION|powershell|ExecutionPolicy/);
  const run = native.split('        run: |\n')[1]?.split('\n').map(line => line.trim()).filter(Boolean);
  assert.deepEqual(run, [
    'set -euo pipefail',
    'node scripts/prepare-core-identity.mjs --platform=win --directory="$POLARIS_CORE_CARRIER_DIR" --mode=verify',
    'cargo test --locked -p polaris-windows-cleaner --lib -- --nocapture 2>&1 | tee native-dns-ci-evidence/c05-native-fixtures.log',
    'export POLARIS_C05_NATIVE_LOG="$PWD/native-dns-ci-evidence/c05-native-fixtures.log"',
    'node --test scripts/nsis-core-sweep.test.mjs scripts/nsis-helper-cleanup.test.mjs',
  ]);
  const build = extractCiStepBlock(ci, CANDIDATE_BUILD_STEP);
  assert.equal(build.match(/^\s*if: (.+)$/m)?.[1],
    "${{ needs.cheap_format.outputs.app_native_compile_only != 'true' && (needs.cheap_format.outputs.native_dns_candidate != '' || needs.cheap_format.outputs.native_dns_default == 'true' || needs.cheap_format.outputs.native_dns_formal == 'true') }}");
  const phases = [
    'cargo build --locked -p polaris-helper --bin polaris-helper',
    'cp target/debug/polaris-helper.exe native-dns-ci-evidence/polaris-helper.exe',
    'POLARIS_WINDOWS_INSTALL_ANCHOR="$(node scripts/prepare-core-identity.mjs --platform=win',
    'export POLARIS_WINDOWS_INSTALL_ANCHOR',
    'cargo build --locked -p polaris-windows-cleaner --bin polaris-cleaner',
    'node scripts/prepare-core-identity.mjs --platform="$POLARIS_NATIVE_DNS_PLATFORM"',
  ].map(marker => { const at = build.indexOf(marker); assert(at >= 0, marker); return at; });
  assert(phases.every((at, i) => i === 0 || phases[i - 1] < at));
  assert(build.includes('--mode=maintenance-anchor --helper-binary=native-dns-ci-evidence/polaris-helper.exe'));
  assert(!build.includes('export POLARIS_WINDOWS_INSTALL_ANCHOR="$('), 'assignment must propagate the child status');
  const before = ci.indexOf('- name: Verify all candidate payloads and compiled identity before workspace build');
  const workspace = ci.indexOf('id: workspace_tests');
  const consumer = ci.indexOf(`- name: ${CI_ONLY_WINDOWS_INSTALLER_STEP}`);
  assert(ci.indexOf(`- name: ${CANDIDATE_BUILD_STEP}`) < before && before < workspace && workspace < consumer);
  assert(!ci.includes('Windows installer cleanup fixtures (native PowerShell)'));
}
test('native maintenance consumer is after full producer admission, not the old early PS step', () => {
  const { ci } = loadStripped();
  assertWindowsMaintenanceConsumer(ci);
  const native = extractCiStepBlock(ci, CI_ONLY_WINDOWS_INSTALLER_STEP);
  for (const [from, to] of [
    ["runner.os == 'Windows'", "runner.os == 'Linux'"],
    ['--mode=verify', '--mode=prepare'],
    ['--locked -p polaris-windows-cleaner --lib -- --nocapture', '-p polaris-helper --lib -- --nocapture'],
    ['--lib -- --nocapture', '--lib -- --no-run'],
    ['c05-native-fixtures.log', 'old-fixture.log'],
    ['export POLARIS_C05_NATIVE_LOG=', 'export OMITTED_LOG='],
    ['scripts/nsis-helper-cleanup.test.mjs', 'scripts/omitted.test.mjs'],
    ['shell: bash', 'continue-on-error: true\n        shell: bash'],
    ['set -euo pipefail', 'set -eu'],
  ]) {
    assert(native.includes(from));
    assert.throws(() => assertWindowsMaintenanceConsumer(ci.replace(native, native.replaceAll(from, to))));
  }
  const build = extractCiStepBlock(ci, CANDIDATE_BUILD_STEP);
  for (const from of [
    'cp target/debug/polaris-helper.exe native-dns-ci-evidence/polaris-helper.exe',
    '--mode=maintenance-anchor --helper-binary=native-dns-ci-evidence/polaris-helper.exe',
    'cargo build --locked -p polaris-windows-cleaner --bin polaris-cleaner',
    'node scripts/prepare-core-identity.mjs --platform="$POLARIS_NATIVE_DNS_PLATFORM"',
  ]) assert.throws(() => assertWindowsMaintenanceConsumer(ci.replace(build, build.replace(from, 'OMITTED'))));
  const moved = ci.replace(native, '').replace(`      - name: ${CANDIDATE_BUILD_STEP}`,
    native + `\n      - name: ${CANDIDATE_BUILD_STEP}`);
  assert.throws(() => assertWindowsMaintenanceConsumer(moved), 'consumer must follow the same producer admission');
});
