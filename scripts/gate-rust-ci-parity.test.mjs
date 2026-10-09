import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

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

/** 抠出 ci.yml 里 `- name: <stepName>` 到下一个同缩进 `- name:` 之间的整段（已剥注释）。 */
function extractCiStepBlock(strippedYaml, stepName) {
  const lines = strippedYaml.split('\n');
  const startIdx = lines.findIndex((l) => l.trim() === `- name: ${stepName}`);
  assert.ok(startIdx !== -1, `ci.yml 里找不到步骤 "- name: ${stepName}"`);
  let endIdx = lines.length;
  for (let i = startIdx + 1; i < lines.length; i++) {
    if (/^\s{6}- name:/.test(lines[i])) {
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

// 5 个门：ci.yml 的 step 名 → gate-rust.sh 的 run_gate 名 → 若该步骤有 env 限定则给出变量名。
const GATES = [
  { ci: 'Check formatting', sh: 'fmt', envVar: null },
  { ci: 'Clippy (deny warnings)', sh: 'clippy', envVar: 'RUSTFLAGS' },
  { ci: 'Rustdoc documentation invariants (deny four lints)', sh: 'rustdoc', envVar: 'RUSTDOCFLAGS' },
  { ci: 'Build', sh: 'build', envVar: null },
  { ci: 'Test', sh: 'test', envVar: null },
];

function loadStripped() {
  const ciRaw = readFileSync(join(ROOT, '.github/workflows/ci.yml'), 'utf8');
  const shRaw = readFileSync(join(ROOT, 'scripts/gate-rust.sh'), 'utf8');
  return { ci: stripComments(ciRaw), sh: stripComments(shRaw) };
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
const MIRRORED_BUT_NOT_VERBATIM = [
  'Cross-check platform targets (cfg(windows) / cfg(macos) / cfg(android) / cfg(ios) 分支)',
  'Cross-target exemptions must still be necessary',
  ANDROID_FACE_STEP,
];

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
test('ci.yml 里跑 cargo 的步骤，要么在 GATES 里、要么在非逐字镜像名单里', () => {
  const { ci } = loadStripped();
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
    if (isGate) cargoSteps.push(m[1]);
  }
  assert.ok(cargoSteps.length > 0, '一个跑 cargo 的步骤都没抠到 —— 取材面是空的，本断言恒真');

  const covered = new Set([...GATES.map((g) => g.ci), ...MIRRORED_BUT_NOT_VERBATIM]);
  const orphans = cargoSteps.filter((n) => !covered.has(n));
  assert.deepEqual(
    orphans,
    [],
    `ci.yml 有跑 cargo 的步骤既不在 GATES 也不在非逐字镜像名单里：${JSON.stringify(orphans)}\n` +
      '要么把它加进 scripts/gate-rust.sh 与 GATES（可逐字对拍的单行命令），' +
      '要么（多行 shell 块，抠不出单行）加进 MIRRORED_BUT_NOT_VERBATIM 并为它补一条针对性对拍。'
  );

  const stale = MIRRORED_BUT_NOT_VERBATIM.filter((n) => !cargoSteps.includes(n));
  assert.deepEqual(stale, [], `非逐字镜像名单里有 ci.yml 已经没有的步骤（名单陈旧）：${JSON.stringify(stale)}`);
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
