#!/usr/bin/env node
// macos-nested-code.mjs —— `.app` 里 `Contents/Resources` 之下的 Mach-O 逐个下判断。
//
// 用法：
//   node scripts/macos-nested-code.mjs <Polaris.app> <源内核文件>
//   <源内核文件> = 本次打包取用的那份 `resources/mac-<arch>/sing-box`（必填，没有缺省值）。
//   封印整个 bundle 前和打开最终产物后各做一次只读核验。
//
// 为什么需要它：
//   内核 `sing-box` 与 `polaris-helper` 住在 `Contents/Resources/_up_/resources/mac-<arch>/`，应用直接
//   执行包内这一份内核。Apple 的资源规则把 `Resources/` 下的文件按**资源**封进 bundle 的哈希表，不当
//   嵌套代码逐个签；照此 bundle 那一次 `codesign --deep` 封印与 `--verify --deep` 都不替这两个文件
//   各自的签名作证（这一条取自规则文本，本仓没有在 runner 上单独实测）。所以这里逐个判断，不借 bundle
//   的封印替它们作证。
//
// ── 三类文件，三种处置（按文件名分流；分流点只有下面 `switch (basename(file))` 一处）──
//
//   1. 内核 `sing-box` —— **只校验，不签、不改写**。
//      内核字节要与源码构建回执里的哈希对得上，打包链上任何一次改写（含补签）都会让它对不上。
//      判据两条，都必须成立：
//        a. 与 <源内核文件> 逐字节相同。这是「打包过程没有改写内核」的正面断言 —— 不是靠
//           「本脚本没调签名命令」反推：bundle 封印、dmg 转换等别的步骤同样在射程内。
//        b. 通过发布核契约验证。在 macOS 上，发布核验证会用 `codesign --verify --strict` 观察实际签名，
//           并与契约中的签名状态和 CDHash 核对。纯 x86_64 且明确未签名时放行并留 notice。
//           依据：Intel 上执行 Mach-O 不要求代码签名；arm64 上没有有效签名的 Mach-O 一执行就被系统
//           终止，故 arm64（含 universal）没有这条例外。「有签名但验不过」在任何架构上都判失败。
//      内核验不过 ⇒ 打包失败，不在这里修：签名属于内核发布产物那一步（fetch-core 消费时由
//      `scripts/desktop-core/source-graph.mjs` 的 observeMacCodeSignature 观测并记入收据）。
//
//   2. `polaris-helper` —— 编译 App 之前已签并冻结。这里只校验源/载体
//      字节与签名；验不过即拒绝，不在包后补签，也不改写安装身份。
//
//   3. 其它 Mach-O（包内容白名单目前不允许出现）—— 只校验，验不过即失败，不替来历不明的文件签名。
//
// 正面断言：扫完必须确实见过 `sing-box` 与 `polaris-helper` 两个 Mach-O。目录不对、文件不是
//   Mach-O、枚举一个都没给出来，都会让「逐个验」变成「一个都没验」而照样 rc=0。
//
// 符号链接：枚举不跟随链接，链接本身不算文件 ⇒ 指向包外的链接不会被当成包内文件去验；
//   `sing-box` 若是一条链接，结果是「没有扫到」而失败。
//
// 这里验不了的（只能在 macOS runner / 真机上成立）：真 `codesign` 对 linker-signed ad-hoc 签名的
//   判定、`--deep` 是否触碰 `Resources/` 下的 Mach-O（由判据 1a 在 runner 上间接回答）、带
//   `com.apple.quarantine` 时系统对包内未签名 x86_64 内核的放行与否。
import { spawnSync } from 'node:child_process';
import { existsSync, lstatSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { basename, dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { verifyPackagedRelease } from './desktop-core/release-consumer.mjs';
import { verifySelectedCoreManifest } from './lib/core-identity.mjs';

const fail = (message, status = 1) => { throw Object.assign(new Error(message), { status }); };
// Unreadable bytes count as different, as `cmp -s` reports them.
const same = (a, b) => { try { return readFileSync(a).equals(readFileSync(b)); } catch { return false; } };

export function main(argv, run = spawnSync, verifyRelease = verifyPackagedRelease, host = process.platform) {
  try {
    if (argv.length !== 2) fail('用法 macos-nested-code.mjs <App.app> <源内核文件>', 2);
    const [app, coreRef] = argv;
    // Absolute paths keep a leading `-` from reaching file/codesign as an option.
    const bundle = resolve(app), res = join(bundle, 'Contents', 'Resources');
    if (!app || !existsSync(res) || !statSync(res).isDirectory()) fail(`不是 app bundle（缺 Contents/Resources）：${app}`, 2);
    if (!coreRef || !existsSync(coreRef) || !lstatSync(coreRef).isFile()) {
      fail(`第二个参数必须是源内核文件（普通文件，非链接）：'${coreRef}'`, 2);
    }
    const tool = (command, args) => {
      const result = run(command, args, { encoding: 'utf8' });
      if (result.error?.code === 'ENOENT') fail(`找不到命令 ${command}（本脚本只在带 Xcode 命令行工具的 macOS 上有意义）`, 2);
      return result;
    };
    const verifySignature = (file, message) => {
      const result = tool('codesign', ['--verify', '--strict', file]);
      if (result.status !== 0) {
        console.error(`${result.stdout ?? ''}${result.stderr ?? ''}`.trimEnd());
        fail(message);
      }
    };

    // 文件清单先完整枚举再判定：中途失败（某个子目录读不了）时这里就失败，而不是带着半份清单继续。
    const files = [];
    const walk = (directory) => {
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        if (entry.isDirectory()) walk(join(directory, entry.name));
        else if (entry.isFile()) files.push(join(directory, entry.name));
      }
    };
    try { walk(res); } catch { fail(`枚举 ${res} 失败`); }

    let count = 0, coreSeen = 0, coreUnsigned = 0, helperSeen = 0;
    for (const file of files) {
      const rel = relative(bundle, file);
      const kind = tool('file', ['-b', file]);
      if (kind.status !== 0) fail(`file 无法判断文件类型：${rel}`);
      if (!kind.stdout.startsWith('Mach-O')) continue;
      count++;
      switch (basename(file)) {
        case 'sing-box': {
          if (!same(file, coreRef)) fail(`包内内核与源内核字节不同：${rel} ≠ ${coreRef} —— 打包链上有步骤改写了内核`);
          let signature;
          try {
            const root = resolve(dirname(dirname(coreRef)), '..');
            const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json'), 'utf8'));
            signature = verifyRelease(root, manifest, basename(dirname(coreRef)), file).macCodeSignature;
            // The verifier matches the actual signature to this receipt only on Darwin.
            if (host !== 'darwin' || !signature) {
              throw Error('Packaged Core signature verification requires macOS and a Mac release');
            }
          } catch (error) {
            console.error(error);
            fail(`包内内核未通过发布核验：${rel}`);
          }
          if (signature.state === 'unsigned') {
            console.log(`::notice::macos-nested-code: ${rel} 是没有签名的 x86_64 内核，按原字节放行（Intel 上执行不要求代码签名）`);
            coreUnsigned++;
          } else if (signature.state !== 'signed') fail(`包内内核缺少已核验的签名状态：${rel}`);
          coreSeen++;
          break;
        }
        // helper bytes were signed and frozen before compiling the App's installation identity.
        case 'polaris-helper':
          if (!same(file, join(dirname(coreRef), 'polaris-helper'))) fail(`包内 helper 与 App 编译前冻结的 helper 不同：${rel}`);
          verifySignature(file, `helper 的签名必须在 App 编译前完成：${rel}`);
          helperSeen++;
          break;
        default:
          verifySignature(file, `未登记的 Mach-O 没有有效签名：${rel} —— 本脚本不补签任何 Mach-O`);
      }
    }

    for (const [seen, name] of [[coreSeen, 'sing-box'], [helperSeen, 'polaris-helper']]) {
      if (seen < 1) fail(`Contents/Resources 下没有扫到 Mach-O「${name}」（共扫到 ${count} 个 Mach-O）—— 逐个验没有验到它`);
    }

    // Verify the exact pre-compilation installation metadata and helper/core bytes.
    const coreKey = basename(dirname(coreRef));
    verifySelectedCoreManifest(join(res, '_up_', 'resources', coreKey), dirname(coreRef), coreKey);

    console.log(`macos-nested-code: Contents/Resources 下 ${count} 个 Mach-O 已逐个判定；内核 ${coreSeen} 份与源内核逐字节相同（其中未签名的 x86_64 ${coreUnsigned} 份，其余签名有效），polaris-helper ${helperSeen} 份签名有效`);
    return 0;
  } catch (error) {
    if (error.status === undefined) throw error;
    console.error(`::error::macos-nested-code: ${error.message}`);
    return error.status;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
