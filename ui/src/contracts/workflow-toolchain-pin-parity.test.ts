/**
 * CI 工具链钉扎的守门测试。
 *
 * 2026-08-17 起 protoc / NASM 换成「固定版本 + 固定 URL + sha256」的自装内联步，当时常量在
 * `ci.yml` 与 `package.yml` 各一份，本文件用「两份逐字对拍」防漂。
 * 2026-08-18（CI-4）起 protoc 收敛到 `scripts/fetch-protoc.mjs`：常量只有一处，「两份对拍」
 * 这个失效模式对 protoc 消失，protoc 对拍部分退休，改为守「workflow 真的在调用脚本」与
 * 「脚本自己的钉扎表有牙」。NASM 仍内联两处，对拍照旧。
 *
 * # 判据
 * - protoc（workflow 侧）：两个文件都有真的 `run: node scripts/fetch-protoc.mjs`——
 *   只 grep 步骤名的话，把 run 块掏空、名字留着照样绿；
 * - protoc（脚本侧）：校验走 `createHash` 且比对不符会抛；`PROTOC_EXPECT` 仍经 `GITHUB_ENV` 导出
 *   （断言步的期望值来源，丢了它断言步拿不到期望）；
 * - NASM：全部内联副本的 `NASM_VERSION` 与 sha256 一致，且**每个文件的副本数固定**——一起删空
 *   也是「相等」，固定条数让删空转红。2026-10-07 `ci.yml` 拆成并行 job 后副本数是
 *   ci.yml 2（lint / test 两个矩阵 job 各自要在 Windows 上编 btls-sys）+ package.yml 1；
 * - 每一份 NASM 副本的 sha256 都真在校验（`sha256sum -c` 的条数 = 副本数）而不是摆着看；
 *   装配步不靠 `command -v` 探测选工具；**每一处**装 protoc 的地方后面都有 PATH 断言
 *   （`[ "$got" = "$PROTOC_EXPECT" ]` 的条数 = `run: node scripts/fetch-protoc.mjs` 的条数）。
 *
 * # 这门抓不到什么（别当成「工具链钉扎都验过了」）
 * - **sha256 对不对**：只保证两处相同 / 表内形齐。两处一起写错，本门全绿；
 *   Linux 腿真正会红的是脚本里的 createHash 比对，win/mac 腿只有 CI 真跑才知道。
 * - **版本选得对不对**：选 35.1 的依据在 `scripts/fetch-protoc.mjs` 头注（一次性实测结论）。
 * - **URL 还在不在**：资产被上游删除只有真下载才知道。
 * - pnpm 在 **win/mac 打包腿**上的行为（CI-2 统一到 10 后，首个全矩阵之前的残余风险，登记在案）。
 */

import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO_ROOT = fileURLToPath(new URL('../../..', import.meta.url));

const WORKFLOWS = ['ci.yml', 'package.yml'] as const;

/** 每个 workflow 里 NASM 内联副本的份数（见文件头「NASM」条）。 */
const NASM_COPIES = { 'ci.yml': 2, 'package.yml': 1 } as const;

function read(workflow: string): string {
  return readFileSync(join(REPO_ROOT, '.github/workflows', workflow), 'utf8');
}

/** 取 `NAME: '<value>'`（workflow step 的 env 钉扎）。 */
function envPin(src: string, name: string): string[] {
  return [...src.matchAll(new RegExp(`^\\s*${name}: '([^']+)'$`, 'gm'))].map((m) => m[1]);
}

/** 取独立成行的 `sha256='<64 位十六进制>'`（NASM 那份）。 */
function standaloneShaPins(src: string): string[] {
  return [...src.matchAll(/^\s*sha256='([0-9a-f]{64})'$/gm)].map((m) => m[1]).sort();
}

describe('CI 工具链钉扎守门', () => {
  const sources = Object.fromEntries(WORKFLOWS.map((w) => [w, read(w)])) as Record<
    (typeof WORKFLOWS)[number],
    string
  >;
  // CI-4 后 protoc 常量的唯一真值点；断言读的是源码文本（与其它 workflow 断言同款手法）。
  const fetchProtoc = readFileSync(join(REPO_ROOT, 'scripts/fetch-protoc.mjs'), 'utf8');

  it('CI test job 检出 Windows 目录内核源仓：限 Windows、精确 ref、独立路径、不留凭据', () => {
    const ci = read('ci.yml').replace(/^\s*#.*$/gm, '');
    const job = (name: string) => ci.split(`\n  ${name}:\n`)[1].split(/\n {2}[a-z][a-z0-9_-]*:\n/)[0];
    const kernelCheckout = job('test').split('- name: Checkout exact Windows directory producer source\n')[1]
      .split(/\n {6}- (?:name:|uses:)/)[0];
    expect(kernelCheckout).toContain("runner.os == 'Windows'");
    expect(kernelCheckout).toContain("steps.windows_directory_source.outcome == 'success'");
    expect(kernelCheckout).toContain('repository: polaris-arch/sing-box-polaris');
    expect(kernelCheckout).toContain('ref: ${{ steps.windows_directory_source.outputs.source }}');
    expect(kernelCheckout).toContain('path: .windows-directory-kernel');
    expect(kernelCheckout).toContain('persist-credentials: false');
    expect(kernelCheckout).toContain('fetch-depth: 1');
  });

  it('fetch-protoc.mjs 的校验与 CI 装配线还在（钉执行形，防被注释/import/log 行喂饱）', () => {
    // 三条都钉「执行形」而不是裸词：CI-4 复审实证过裸词的喂饱路径——`createHash` 被 import 行
    // 喂饱、`PROTOC_EXPECT=libprotoc` 被 console.log 行喂饱、`GITHUB_PATH` 被注释喂饱、
    // `sha256 不符` 词在 throw 被改成 console.error 后仍绿（那等于校验静默放行，供应链牙全失效）。
    expect(
      fetchProtoc.includes('throw new Error(`sha256 不符'),
      'sha256 不符的处置必须是 throw（经 catch 转 exit 1）—— 换成 console.error 就成了静默放行'
    ).toBe(true);
    expect(
      fetchProtoc.includes("createHash('sha256')"),
      '脚本丢了 sha256 校验调用 —— 常量还在、校验没了，钉扎退化成装饰'
    ).toBe(true);
    expect(
      fetchProtoc.includes('appendFileSync(process.env.GITHUB_ENV, `PROTOC_EXPECT='),
      'PROTOC_EXPECT 不再经 GITHUB_ENV 真写入 —— 断言步拿不到期望值，安装步可静默 no-op'
    ).toBe(true);
    expect(
      fetchProtoc.includes('appendFileSync(process.env.GITHUB_PATH'),
      'CI 的 PATH 注册线没了 —— 装了但断言步解析不到；此处防整段被误删'
    ).toBe(true);
  });

  it('pnpm 版本两处一致且都已精确钉扎', () => {
    // UI 门、出包腿和本地 Corepack 入口必须同版；分家会让门验过的安装语义与产物不一致。
    const ui = envPin(read('ui.yml'), 'PNPM_VERSION');
    const pkg = envPin(sources['package.yml'], 'PNPM_VERSION');
    expect(ui, 'ui.yml 里没有精确钉扎的 PNPM_VERSION —— pnpm 又浮动了？').toHaveLength(1);
    expect(pkg, 'package.yml 里没有精确钉扎的 PNPM_VERSION —— 出包腿工具链不可复现').toHaveLength(1);
    expect(
      pkg[0],
      'ui 门与出包腿的 pnpm 版本不同：两侧装出来的 node_modules 不是一回事（见 CI-2）'
    ).toBe(ui[0]);
    const packageJson = JSON.parse(readFileSync(join(REPO_ROOT, 'ui/package.json'), 'utf8')) as {
      packageManager?: string;
    };
    expect(packageJson.packageManager).toBe(`pnpm@${ui[0]}`);

    const uiWorkflow = read('ui.yml');
    expect(uiWorkflow.indexOf('actions/setup-node@v7')).toBeLessThan(
      uiWorkflow.indexOf('- name: Setup pnpm (pinned)')
    );
    // 版本同还不够，install 形态也要同（复审 Low 的补刀）：版本一致而一侧掏掉
    // `--frozen-lockfile` / 换安装器，lockfile 就能静默漂移——同一个坑的另一半。
    // 钉整段执行形而不是步骤名：掏空 run 块、名字留着照样绿是本文件记载过的失效形态。
    expect(
      read('ui.yml').includes('pnpm install --frozen-lockfile'),
      'ui.yml 的 install 丢了 --frozen-lockfile —— 门腿可以装出 lockfile 之外的依赖'
    ).toBe(true);
    expect(
      sources['package.yml'].includes('pnpm --dir ui install --frozen-lockfile'),
      'package.yml 的 install 丢了 --frozen-lockfile —— 出包腿可以装出门验过之外的依赖'
    ).toBe(true);
  });

  it('NASM 版本与 sha256 在全部内联副本之间一致，且每个文件的副本数固定', () => {
    const versions: string[] = [];
    const shas: string[] = [];
    for (const workflow of WORKFLOWS) {
      const ver = envPin(sources[workflow], 'NASM_VERSION');
      const sha = standaloneShaPins(sources[workflow]);
      // 份数固定：删掉一份（那个 job 的 Windows 腿从此用不上钉扎的 NASM）或悄悄多抄一份都红。
      expect(ver, `${workflow} 的 NASM_VERSION 份数不对`).toHaveLength(NASM_COPIES[workflow]);
      expect(sha, `${workflow} 的独立 sha256（NASM）份数不对`).toHaveLength(NASM_COPIES[workflow]);
      versions.push(...ver);
      shas.push(...sha);
    }
    // 取材面自检：一份都没读到时「全部相等」恒真。
    expect(versions.length).toBe(NASM_COPIES['ci.yml'] + NASM_COPIES['package.yml']);
    expect(new Set(versions).size, `NASM 版本在各副本之间漂移了：${versions.join(' / ')}`).toBe(1);
    expect(new Set(shas).size, `NASM sha256 在各副本之间漂移了：${shas.join(' / ')}`).toBe(1);
  });

  it.each(WORKFLOWS)('%s 里每一份 NASM 的 sha256 都真在校验，不是摆着看', (workflow) => {
    const src = sources[workflow];
    // 按份数判而不是按「文件里有没有」：ci.yml 现在有两份，只留一条校验命令时
    // `includes` 照样为真，而另一份的常量就成了摆设。
    expect(
      src.match(/^\s*echo "\$\{sha256\}  \$\{zip\}" \| sha256sum -c -$/gm) ?? [],
      `${workflow} 里 NASM 的 sha256sum 校验命令条数与副本数不符 —— 有常量还在、校验没了的副本`
    ).toHaveLength(NASM_COPIES[workflow]);
  });

  // `scripts/lib/extract-zip.mjs` 头注那条 🔴：解压器/校验器**按平台写死，不写「先试 A 失败退 B」**
  // —— 静默 fallback 会让「A 其实不在」永远不被观测到，换 runner 镜像时原样复发且报错点已漂走。
  // 这两步第一版正是写成了 `command -v` 探测（2026-08-17 复审时改掉），本条守它不回潮。
  // 只钉这三个**探测**形态：断言步里的 `command -v protoc` / `command -v nasm` 是**报告**用哪个
  // 二进制、不做分支，属于要保留的东西，故不在射程内。
  it.each(WORKFLOWS)('%s 的装配步不靠 `command -v` 探测选工具', (workflow) => {
    const src = sources[workflow];
    for (const probe of ['command -v unzip', 'command -v 7z', 'command -v sha256sum']) {
      expect(
        src.includes(probe),
        `${workflow} 出现了 \`${probe}\` —— 工具选择又变成了静默 fallback，` +
          `见 scripts/lib/extract-zip.mjs 头注那条 🔴：判据要按平台写死并把实际用的工具打进日志`
      ).toBe(false);
    }
  });

  it.each(WORKFLOWS)('%s 每一处装 protoc 之后都断言 PATH 上解析到的就是钉扎的那份', (workflow) => {
    const src = sources[workflow];
    const installs = src.match(/^ *run: node scripts\/fetch-protoc\.mjs\s*$/gm) ?? [];
    // 匹配断言里**真正的比较**，不是步骤名：只 grep 步骤名的话，把 run 块掏空、名字留着照样绿。
    const asserts = src.match(/^\s*\[ "\$got" = "\$PROTOC_EXPECT" \] \|\| \{$/gm) ?? [];
    expect(installs.length, `${workflow} 里一处 protoc 安装都没读到 —— 取材面塌了`).toBeGreaterThan(0);
    // 按条数配对而不是按「文件里有没有」：装 protoc 的 job 变多之后，只要还剩一条断言
    // `includes` 就为真，其余 job 的安装步变成静默 no-op 也没人知道。
    expect(
      asserts.length,
      `${workflow} 有 ${installs.length} 处装 protoc，却只有 ${asserts.length} 条 PATH 断言 —— ` +
        '某个 job 的安装步变成静默 no-op 也没人知道'
    ).toBe(installs.length);
  });
});
