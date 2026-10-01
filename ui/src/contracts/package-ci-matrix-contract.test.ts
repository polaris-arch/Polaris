/**
 * package(all) → reusable CI 的矩阵入口契约。
 *
 * GitHub workflow_call 会继承外层 workflow_dispatch 的 event_name/payload。package.yml 的输入叫
 * `platform`，没有 ci.yml 自身手动入口的 `os`；因此 package(all) 调 CI 时
 * `github.event.inputs.os === ''`。空串若只经过 `!= 'all'` 判定，会被误当成具体平台并生成 `[""]`，
 * 最终是 `runs-on: ""` / labels=[] 的永久 pending job（run 32357370395 的真实失败形态）。
 */

import { describe, expect, it, onTestFinished } from 'vitest';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { execFile, spawnSync } from 'node:child_process';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const REPO_ROOT = fileURLToPath(new URL('../../..', import.meta.url));

function workflow(name: string): string {
  return readFileSync(join(REPO_ROOT, '.github/workflows', name), 'utf8');
}

/**
 * 取出 `jobs:` 下某个 job 的整段文本（含其注释），从 `\n  <name>:\n` 到下一个同缩进 job 键为止。
 *
 * 为什么必须按 job 切片、而不是对整个文件 `toContain`：判据是「这套 allowlist 挂在**哪个** job 上」。
 * 2026-09-04 打包腿与质量门改并行，两门的 allowlist 从 `package` 搬到了发布写入腿；如果断言只看
 * 全文，搬家前后同样通过——门在，但对「挂错 job」这件事完全没有牙。
 */
function jobBlock(src: string, name: string): string {
  const start = src.indexOf(`\n  ${name}:\n`);
  if (start < 0) throw new Error(`package.yml 里找不到 job '${name}' —— 取材面塌了，本门此刻没有判据`);
  const rest = src.slice(start + 1);
  const next = rest.slice(1).search(/\n {2}[a-z][a-z0-9_-]*:\n/);
  return next < 0 ? rest : rest.slice(0, next + 1);
}

/** 取出 job 内一个具名 step，避免跨 step 的同名环境变量让接线断言假绿。 */
function stepBlock(src: string, name: string): string {
  const start = src.indexOf(`\n      - name: ${name}\n`);
  if (start < 0) throw new Error(`job 里找不到 step '${name}' —— 取材面塌了，本门此刻没有判据`);
  const rest = src.slice(start + 1);
  const next = rest.slice(1).search(/\n {6}- (?:name:|uses:)/);
  return next < 0 ? rest : rest.slice(0, next + 1);
}

/**
 * 剥掉整行注释，只留 job 的可执行部分。
 *
 * 为什么必须剥：本文件的判据字符串（`needs.package.result == 'success'` 等）在同一段 YAML 的
 * **解释性注释里也逐字出现**。不剥注释就是「判据被自己污染」——把 `if:` 里的那条判据整行删掉，
 * 断言仍然从注释里读到同一串字而通过。2026-09-04 的变异实测确实出现过这个假绿。
 */
function executable(block: string): string {
  return block
    .split('\n')
    .filter((line) => !/^\s*#/.test(line))
    .join('\n');
}

describe('package 全平台前置 CI 的矩阵输入', () => {
  it('空 os 必须回到完整矩阵，只有显式非 all 的 os 才能走单平台', () => {
    const ci = workflow('ci.yml');
    expect(ci).toMatch(
      /github\.event_name == 'workflow_dispatch'\s*&& github\.event\.inputs\.os != ''\s*&& github\.event\.inputs\.os != 'all'/s,
    );
    expect(ci).toContain(`fromJSON(format('["{0}"]', github.event.inputs.os))`);
    expect(ci).toContain(`fromJSON('["ubuntu-22.04","windows-2022","macos-14"]')`);
  });

  it('package 的 dispatch 输入确实只有 platform，并复用 ci.yml 作全平台门', () => {
    const pkg = workflow('package.yml');
    const dispatch = pkg.slice(pkg.indexOf('workflow_dispatch:'), pkg.indexOf('# 最小权限'));
    expect(dispatch).toContain('platform:');
    expect(dispatch).not.toMatch(/^\s+os:/m);
    expect(pkg).toContain('uses: ./.github/workflows/ci.yml');
    expect(pkg).toContain("needs.setup.outputs.full == 'true'");
    expect(pkg).toContain('inputs.skip_quality_gates != true');
  });

  it('独立门与 Package 复用门必须按调用方隔离并发组，不能互相取消制造假红', () => {
    expect(workflow('ci.yml')).toContain(
      'group: ci-${{ github.workflow }}-${{ github.ref }}',
    );
    expect(workflow('ui.yml')).toContain(
      'group: ui-${{ github.workflow }}-${{ github.ref }}',
    );
  });

  it('切片器本身有效：两个 job 都切得出非平凡的块', () => {
    // 正面自检。少了这条，jobBlock 若因缩进/命名变化退化成空串，下面两条全部真空通过。
    const pkg = workflow('package.yml');
    for (const name of ['package', 'release_desktop', 'android_release', 'release']) {
      const block = jobBlock(pkg, name);
      expect(block.length, `job '${name}' 的切片过短，取材面可疑`).toBeGreaterThan(200);
      expect(block, `job '${name}' 的切片没包含它自己的 needs`).toContain('needs:');
      // 切片不得越界到下一个 job。
      expect(block).not.toMatch(
        /\n {2}(?!$)(?:package|release_desktop|android_release|release):\n(?![\s\S]*^$)/m,
      );
    }
  });

  it('发布腿只接受成功或按设计跳过的质量门，取消态不得被当成可放行', () => {
    const pkg = workflow('package.yml');
    const release = executable(jobBlock(pkg, 'release_desktop'));

    // 剥注释自检（正面断言）：剥完必须还剩下 if 与 needs，否则下面全是真空通过。
    expect(release, '剥注释后 release 块空了 —— 取材面塌了').toMatch(/^\s+if:/m);
    expect(release).toMatch(/^\s+needs:/m);
    expect(release, '剥注释没生效 —— 块里仍有整行注释').not.toMatch(/^\s*#/m);

    // 判据挂在第一次 release mutation 上（桌面草稿 job）；门未过时连草稿都不写。
    expect(release).toContain("needs: [setup, ci, ui, package]");
    expect(release).toContain(
      "needs.ci.result == 'success' || needs.ci.result == 'skipped'",
    );
    expect(release).toContain(
      "needs.ui.result == 'success' || needs.ui.result == 'skipped'",
    );

    // always() 关掉了「needs 全绿才跑」的默认语义，故这条必须被显式写回，否则打包腿挂了也照发。
    expect(release).toContain('always()');
    expect(
      release,
      'release 的 if 里加了 always() 却没显式要求 package 成功 —— 打包腿全挂也会发布',
    ).toContain("needs.package.result == 'success'");

    // cancelled 也满足「不等于 failure」：全文范围内都不许出现这种放行式判据。
    expect(pkg).not.toContain("needs.ci.result != 'failure'");
    expect(pkg).not.toContain("needs.ui.result != 'failure'");
    expect(pkg).not.toContain("needs.package.result != 'failure'");
  });

  it('打包腿与质量门并行：package 不得把 ci/ui 写进 needs（写了就退回串行）', () => {
    const pkg = workflow('package.yml');
    const pkgJob = executable(jobBlock(pkg, 'package'));
    expect(pkgJob, '剥注释后 package 块空了 —— 取材面塌了').toMatch(/^\s+if:/m);
    expect(pkgJob).toContain('needs: [setup, desktop_core]');
    const coreJob = executable(jobBlock(pkg, 'desktop_core'));
    expect(coreJob).toContain('needs: setup');
    expect(coreJob).toContain('uses: ./.github/workflows/desktop-core.yml');
    expect(coreJob).toContain('candidate: ${{ needs.setup.outputs.candidate }}');
    expect(pkgJob).toContain("needs.setup.result == 'success'");
    expect(pkgJob).toContain("needs.desktop_core.result == 'success'");
    expect(pkgJob).toContain("needs.desktop_core.result == 'skipped' && inputs.core_bundle_artifact != ''");
    // needs 即等待。package 一旦能读到 needs.ci/needs.ui，就说明它在等两门，并行拓扑已被推翻。
    expect(
      pkgJob,
      'package job 引用了 needs.ci / needs.ui —— 它又串回门后面了',
    ).not.toMatch(/needs\.(ci|ui)\./);
  });

  it('复用 bundle 的真实 setup 前置只接受同 candidate、run 和 attempt', () => {
    const resolve = stepBlock(jobBlock(workflow('package.yml'), 'setup'), 'Resolve platform matrix');
    expect(resolve).toContain('EXPECTED_CANDIDATE: ${{ github.event.pull_request.head.sha || github.sha }}');
    expect(resolve).toContain('CORE_CANDIDATE: ${{ inputs.core_candidate || github.event.pull_request.head.sha || github.sha }}');
    expect(resolve).toContain('CORE_RUN_ID: ${{ github.run_id }}');
    expect(resolve).toContain('CORE_RUN_ATTEMPT: ${{ github.run_attempt }}');
    const runAt = resolve.indexOf('\n        run: |\n');
    const matrixAt = resolve.indexOf('\n          all=', runAt);
    expect(runAt).toBeGreaterThan(0);
    expect(matrixAt).toBeGreaterThan(runAt);
    // Only the actual identity prelude runs: no matrix tools, producers or workflow jobs.
    const script = resolve.slice(runAt + '\n        run: |\n'.length, matrixAt)
      .split('\n').map((line) => line.slice(10)).join('\n');
    const candidate = 'a'.repeat(40);
    const base = {
      CORE_CANDIDATE: candidate, EXPECTED_CANDIDATE: candidate,
      CORE_RUN_ID: '12', CORE_RUN_ATTEMPT: '3',
      CORE_BUNDLE_ARTIFACT: `desktop-core-bundle-${candidate}-12-3`,
    };
    const cases: [string, Record<string, string>, number][] = [
      ['exact reuse', {}, 0], ['standalone producer', { CORE_BUNDLE_ARTIFACT: '' }, 0],
      ['different candidate', { CORE_CANDIDATE: 'b'.repeat(40) }, 1],
      ['malformed candidate', { CORE_CANDIDATE: 'not-a-commit' }, 1],
      ['previous run', { CORE_BUNDLE_ARTIFACT: `desktop-core-bundle-${candidate}-11-3` }, 1],
      ['previous attempt', { CORE_BUNDLE_ARTIFACT: `desktop-core-bundle-${candidate}-12-2` }, 1],
    ];
    for (const [name, changed, status] of cases) {
      const result = spawnSync('bash', ['-c', script], { env: { ...process.env, ...base, ...changed }, encoding: 'utf8' });
      expect(result.status, `${name}: ${result.stderr}`).toBe(status);
    }
  });

  it('发布 DAG 必须是桌面草稿 → 签名 APK → 全量核验公开，且取消态一律不放行', () => {
    const pkg = workflow('package.yml');
    const desktopAt = pkg.indexOf('\n  release_desktop:\n');
    const androidAt = pkg.indexOf('\n  android_release:\n');
    const finalAt = pkg.indexOf('\n  release:\n');
    expect(desktopAt).toBeGreaterThan(0);
    expect(androidAt).toBeGreaterThan(desktopAt);
    expect(finalAt).toBeGreaterThan(androidAt);

    const desktop = executable(jobBlock(pkg, 'release_desktop'));
    const android = executable(jobBlock(pkg, 'android_release'));
    const release = executable(jobBlock(pkg, 'release'));

    expect(desktop).toContain('release_tag: ${{ steps.release_identity.outputs.tag }}');
    expect(desktop).toContain('release_sha: ${{ steps.release_identity.outputs.sha }}');
    expect(android).toContain('needs: release_desktop');
    expect(android).toContain('permissions:\n      contents: write');
    expect(android).toContain('uses: ./.github/workflows/android.yml');
    expect(android).toContain('publish_release: true');
    expect(android).toContain(
      'release_tag: ${{ needs.release_desktop.outputs.release_tag }}',
    );
    expect(android).toContain('secrets: inherit');

    expect(release).toContain('needs: [release_desktop, android_release]');
    expect(release).toContain("needs.release_desktop.result == 'success'");
    expect(release).toContain("needs.android_release.result == 'success'");
    expect(release).not.toContain("needs.android_release.result != 'failure'");
  });

  it('只有最终 job 能公开；已有公开 release 与 tag/commit 漂移都在写入前失败', () => {
    const pkg = workflow('package.yml');
    const desktop = executable(jobBlock(pkg, 'release_desktop'));
    const android = executable(jobBlock(pkg, 'android_release'));
    const release = executable(jobBlock(pkg, 'release'));

    expect(executable(pkg).match(/--draft=false/g) ?? []).toHaveLength(1);
    expect(desktop).not.toContain('--draft=false');
    expect(android).not.toContain('--draft=false');
    expect(release).toContain('gh release edit "$TAG" --repo "$GITHUB_REPOSITORY" --draft=false');
    expect(release.indexOf('Verify combined asset set and digests')).toBeLessThan(
      release.indexOf('Promote release to public'),
    );

    expect(desktop).toContain('tag_sha="$(git rev-list -n 1 "$tag")"');
    expect(desktop).toContain(
      'git fetch --force --depth=1 origin "+refs/tags/$tag:refs/tags/$tag"',
    );
    expect(desktop).toContain(
      'version="$(node -p \'require("./src-tauri/tauri.conf.json").version\')"',
    );
    expect(desktop).toContain('[ "$tag" != "v$version" ]');
    expect(desktop).toContain('[ "$tag_sha" != "$GITHUB_SHA" ]');
    expect(desktop).toContain('[ "$is_draft" != "true" ]');
    expect(desktop).not.toContain('[ "$target_commitish" != "$GITHUB_SHA" ]');
    expect(desktop).not.toContain('--target "$GITHUB_SHA"');
    expect(desktop).toContain('targetCommitish=$target_commitish（诊断信息');
    expect(desktop.match(/^\s*verify_remote_tag$/gm) ?? []).toHaveLength(3);
    expect(desktop.indexOf('[ "$is_draft" != "true" ]')).toBeLessThan(
      desktop.indexOf('gh release upload "$tag" "${files[@]}"'),
    );
    expect(desktop).toContain('已不再是同 tag 草稿，拒绝上传桌面资产');
    const desktopUploadAt = desktop.indexOf('gh release upload "$tag" "${files[@]}"');
    const lastDraftReadBeforeUpload = desktop.lastIndexOf(
      'gh release view "$tag"',
      desktopUploadAt,
    );
    expect(lastDraftReadBeforeUpload).toBeGreaterThan(0);
    expect(lastDraftReadBeforeUpload).toBeLessThan(desktopUploadAt);
    expect(release).toContain('EXPECTED_SHA: ${{ needs.release_desktop.outputs.release_sha }}');
    expect(release).toContain('[ "$tag_sha" != "$GITHUB_SHA" ]');
    expect(release.match(/git fetch --force/g) ?? []).toHaveLength(3);
    expect(release).toContain('[ "$remote_tag_sha" != "$GITHUB_SHA" ]');
    expect(release).toContain('.targetCommitish');
    expect(release).not.toContain('[ "$target_commitish" != "$GITHUB_SHA" ]');
    expect(release).not.toContain('targetCommitish <<<"$release_json")" != "$GITHUB_SHA"');
    expect(release).toContain('拒绝覆盖 SHA256SUMS');
  });

  it('Android 发布腿只写同 commit 草稿，并验真实 APK 签名与远端 digest', () => {
    const androidWorkflow = workflow('android.yml');
    const allExecutable = executable(androidWorkflow);
    const releaseCheck = executable(jobBlock(androidWorkflow, 'release_check'));
    const publish = executable(jobBlock(androidWorkflow, 'release-apk'));
    const dispatch = androidWorkflow.slice(
      androidWorkflow.indexOf('  workflow_dispatch:'),
      androidWorkflow.indexOf('\npermissions:'),
    );

    expect(publish).toContain('if: inputs.publish_release');
    expect(dispatch).not.toContain('publish_release:');
    expect(dispatch).not.toContain('release_tag:');
    expect(androidWorkflow).toContain(
      'value: ${{ jobs.release-apk.outputs.signed_apk_sha256 }}',
    );
    expect(
      [androidWorkflow, workflow('package.yml'), workflow('release-risk.yml')]
        .join('\n')
        .match(/publish_release:\s*true/g) ?? [],
    ).toHaveLength(1);
    expect(androidWorkflow).toContain(
      'group: android-${{ github.workflow }}-${{ github.ref }}',
    );
    expect(workflow('package.yml')).toContain(
      "group: package-${{ github.ref }}-${{ inputs.platforms || inputs.platform || github.event.inputs.platform || 'all' }}",
    );
    expect(releaseCheck).toContain('if: inputs.publish_release != true');
    expect(androidWorkflow).not.toContain('\n  apk:\n');
    expect(allExecutable).not.toContain('--debug');
    expect(allExecutable).not.toContain('app-arm64-debug.apk');
    expect(allExecutable).not.toContain('assembleArm64Debug');

    expect(releaseCheck).toContain(
      ':app:assembleArm64Release -PpolarisAllowUnsigned=true',
    );
    expect(releaseCheck).toContain(
      'run: bash scripts/gate-android-release-behavior.sh',
    );
    expect(releaseCheck).toContain(
      'python3 scripts/libbox-patches/verify-receipt.py --apk src-tauri/gen/android/app/build/outputs/apk/arm64/release/app-arm64-release-unsigned.apk --abi arm64-v8a --r8 src-tauri/gen/android/app/build/outputs/mapping/arm64Release',
    );
    expect(releaseCheck).toContain(
      'node scripts/assert-r8-evidence.mjs src-tauri/gen/android/app/build/outputs/mapping/arm64Release',
    );
    // R8 入口随 source-consumption 收敛到 verifier；守住实际委托、必填证据和失败传播。
    const verifier = readFileSync(join(REPO_ROOT, 'scripts/libbox-patches/verify-receipt.py'), 'utf8');
    const consumption = verifier.slice(verifier.indexOf('def consumption('), verifier.indexOf('\ndef main('));
    expect(consumption).toContain("builder.run(['node', str(ROOT / 'scripts/assert-r8-evidence.mjs'), str(r8)], cwd=ROOT)");
    expect(consumption).toContain("builder.run(['node', str(ROOT / 'scripts/verify-apk.mjs'), str(apk), '--abi', abi], cwd=ROOT)");
    const beforePackage = consumption.slice(0, consumption.indexOf('    with zipfile.ZipFile('));
    expect(beforePackage).not.toMatch(/^\s*try:/m);
    expect(consumption).not.toMatch(/^\s*except\b/m);
    expect(verifier).toContain("builder.require(args.r8 is not None, 'APK source consumption requires actual R8 evidence')");
    expect(verifier).toContain('consumption(args.apk.resolve(), args.abi, args.r8.resolve(), receipt, receipt_path)');
    expect(verifier).toMatch(/except \([^\n]*subprocess\.CalledProcessError\) as error:\n\s*print\([^\n]*\n\s*sys\.exit\(1\)/);
    const builder = readFileSync(join(REPO_ROOT, 'scripts/libbox-patches/build.py'), 'utf8');
    const run = builder.slice(builder.indexOf('def run('), builder.indexOf('\ndef require('));
    expect(run).toContain('return subprocess.run(args, cwd=cwd, env=env, check=True, text=True,');
    expect(run).not.toMatch(/^\s*(?:try:|except\b)/m);
    expect(releaseCheck).toContain(
      'run: node scripts/verify-apk.mjs src-tauri/gen/android/app/build/outputs/apk/arm64/release/app-arm64-release-unsigned.apk --abi arm64-v8a',
    );
    expect(releaseCheck).toContain('name: android-release-mapping');
    expect(releaseCheck).not.toContain('gh release upload');
    expect(releaseCheck).not.toContain('actions/upload-artifact@v7\n        with:\n          name: android-apk');

    expect(publish).toContain(
      'git fetch --force --depth=1 origin "+refs/tags/$tag:refs/tags/$tag"',
    );
    expect(publish.match(/git fetch --force/g) ?? []).toHaveLength(2);
    expect(publish).toContain('tag_sha="$(git rev-list -n 1 "$tag")"');
    expect(publish).toContain('[ "$tag_sha" != "$GITHUB_SHA" ]');
    expect(publish).toContain('[ "$is_draft" != "true" ]');
    expect(publish).not.toContain('[ "$target_commitish" != "$GITHUB_SHA" ]');
    expect(publish).toContain('targetCommitish=$target_commitish（仅诊断');
    expect(publish).toContain('[ "$remote_tag_sha" != "$GITHUB_SHA" ]');
    expect(publish.indexOf('[ "$is_draft" != "true" ]')).toBeLessThan(
      publish.indexOf('gh release upload "$tag" "$asset"'),
    );
    expect(publish).toContain('keytool -exportcert -keystore "$jks"');
    expect(publish).toContain(
      "pinned_cert_sha256='22c183cb41d3dcbe6d355ba58d0a012953c5036592b61fb1efacfe3fa36eabd7'",
    );
    expect(publish).toContain('[ "$cert_sha256" != "$pinned_cert_sha256" ]');
    expect(publish).toContain(
      'signature_report="$("$apksigner" verify --verbose --print-certs "$asset")"',
    );
    expect(publish).toContain('[ "${certs[0]:-}" != "$POLARIS_RELEASE_CERT_SHA256" ]');
    expect(publish).toContain(
      'expected_id="$(node -p \'require("./src-tauri/tauri.android.conf.json").identifier\')"',
    );
    expect(publish).toContain('[ "$actual_id" != "$expected_id" ]');
    expect(publish).toContain('run: bash scripts/gate-android-release-behavior.sh');
    expect(publish).toContain(
      'run: node scripts/assert-r8-evidence.mjs src-tauri/gen/android/app/build/outputs/mapping/arm64Release',
    );
    expect(publish).toContain('name: android-release-mapping');
    expect(publish).toContain(
      'node scripts/verify-apk.mjs ${{ steps.asset.outputs.asset }} --abi arm64-v8a',
    );
    expect(publish).toContain(
      'python3 scripts/libbox-patches/verify-receipt.py --apk ${{ steps.asset.outputs.asset }} --abi arm64-v8a --r8 src-tauri/gen/android/app/build/outputs/mapping/arm64Release',
    );
    expect(publish).toContain('expected_sha="${{ steps.asset.outputs.sha256 }}"');
    expect(publish).toContain('[ "$actual_sha" != "sha256:$expected_sha" ]');
    expect(publish).toContain(
      'signed_apk_sha256: ${{ steps.asset.outputs.sha256 }}',
    );
    expect(publish).not.toContain('--draft=false');
  });

  it('Android 资产命名读取实际 Gradle metadata，并与 APK manifest 的双版本对拍', () => {
    const publish = jobBlock(workflow('android.yml'), 'release-apk');
    const asset = stepBlock(publish, '改名成 release 资产名');
    const run = asset.split('        run: |\n')[1];
    expect(run).toBeDefined();
    const good = "package: name='com.polaris2.app' versionCode='1000000' versionName='1.0.0' platformBuildVersionName='16'";
    const goodProps = 'tauri.android.versionName=1.0.0\ntauri.android.versionCode=1000000\n';
    for (const [badging, properties, tag, aaptExit, expected] of [
      [good, goodProps, 'v1.0.0', 0, 0],
      [good.replace("versionName='1.0.0'", "versionName='0.9.0'"), goodProps, 'v1.0.0', 0, 1],
      [good.replace("versionCode='1000000'", "versionCode='999999'"), goodProps, 'v1.0.0', 0, 1],
      [good, goodProps.replace('versionName=1.0.0', 'versionName=0.9.0'), 'v1.0.0', 0, 1],
      [good, goodProps.replace('versionCode=1000000', 'versionCode=999999'), 'v1.0.0', 0, 1],
      [good, goodProps + 'tauri.android.versionName=1.0.0\n', 'v1.0.0', 0, 1],
      [good, goodProps + 'tauri.android.versionCode=1000000\n', 'v1.0.0', 0, 1],
      [good, 'tauri.android.versionName=1.0.0\n', 'v1.0.0', 0, 1],
      [good, 'tauri.android.versionCode=1000000\n', 'v1.0.0', 0, 1],
      [good, null, 'v1.0.0', 0, 1],
      ['', goodProps, 'v1.0.0', 0, 1],
      ["package: name='com.polaris2.app'", goodProps, 'v1.0.0', 0, 1],
      [good + '\n' + good, goodProps, 'v1.0.0', 0, 1],
      [good.replace("versionCode='1000000'", "versionCode='invalid'"), goodProps, 'v1.0.0', 0, 1],
      [good, goodProps, 'v1.0.0', 1, 1],
      [good, goodProps, 'v0.9.0', 0, 1],
    ] as const) {
      const fixture = mkdtempSync(join(tmpdir(), 'polaris-android-asset-'));
      try {
        const app = join(fixture, 'src-tauri/gen/android/app');
        const apkDir = join(app, 'build/outputs/apk/arm64/release');
        const tools = join(fixture, 'sdk/build-tools/36.0.0');
        const output = join(fixture, 'github-output');
        mkdirSync(apkDir, { recursive: true });
        mkdirSync(tools, { recursive: true });
        writeFileSync(join(app, 'build.gradle.kts'), 'android { compileSdk = 36 }\n');
        if (properties !== null) {
          writeFileSync(join(app, 'tauri.properties'), properties);
        }
        const bytes = 'synthetic APK bytes; no Android build';
        writeFileSync(join(apkDir, 'app-arm64-release.apk'), bytes);
        // Only the real asset step's metadata input is synthetic. No SDK or APK
        // command runs; the shell probe returns the manifest fixture verbatim.
        const aapt2 = join(tools, 'aapt2');
        writeFileSync(aapt2, '#!/bin/sh\n[ "$1" = dump ] && [ "$2" = badging ] || exit 2\nprintf \'%s\\n\' "$APK_BADGING"\nexit "$AAPT_EXIT"\n');
        chmodSync(aapt2, 0o755);
        expect(existsSync(join(fixture, 'src-tauri/gen/android/.tauri/tauri.properties'))).toBe(false);
        const script = run.replace(/^ {10}/gm, '').replaceAll(
          '${{ steps.release_identity.outputs.tag }}', tag,
        );
        const result = spawnSync('bash', ['-c', script], {
          cwd: fixture,
          env: { ...process.env, RUNNER_TEMP: fixture, GITHUB_OUTPUT: output,
            ANDROID_HOME: join(fixture, 'sdk'), APK_BADGING: badging, AAPT_EXIT: String(aaptExit) },
          encoding: 'utf8',
        });
        expect(result.status, result.stdout + result.stderr).toBe(expected);
        const renamed = join(fixture, 'polaris-1.0.0-android-arm64.apk');
        expect(existsSync(renamed)).toBe(expected === 0);
        if (expected === 0) {
          expect(readFileSync(renamed, 'utf8')).toBe(bytes);
          expect(readFileSync(output, 'utf8')).toContain('name=polaris-1.0.0-android-arm64.apk\n');
        } else {
          expect(existsSync(output)).toBe(false);
        }
      } finally {
        rmSync(fixture, { recursive: true, force: true });
      }
    }
  });

  it.each(([0, 1, 2, 3] as const).flatMap((index) =>
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9].map((caseIndex) => [index, caseIndex] as const),
  ))('四处远端 digest 裁判 %i 场景 %i 经真实 gh/gojq 解析，且拒绝缺失或漂移的资产', async (index, caseIndex) => {
    const android = stepBlock(jobBlock(workflow('android.yml'), 'release-apk'), '上传成 release 资产');
    const desktop = stepBlock(jobBlock(workflow('package.yml'), 'release_desktop'),
      'Verify draft desktop asset digests against SHA256SUMS');
    const release = jobBlock(workflow('package.yml'), 'release');
    const combined = stepBlock(release, 'Verify combined asset set and digests');
    const promote = stepBlock(release, 'Promote release to public');
    const guards = [
      android.slice(android.indexOf("          row=''")),
      desktop.slice(desktop.indexOf('          raw_all=')),
      combined.slice(combined.indexOf('          raw="$(mktemp)"')),
      promote.slice(promote.indexOf('          raw="$(mktemp)"'), promote.indexOf('          gh release edit')),
    ] as const;
    const guard = guards[index];
    const sha = 'a'.repeat(64);
    let assets: Array<{ name: string; digest?: string | null; state?: string }> = [];
    let requests = 0;
    const server = createServer((request, response) => {
      expect(request.method).toBe('GET');
      expect(request.url).toBe('/release');
      requests += 1;
      response.setHeader('Content-Type', 'application/json');
      response.end(JSON.stringify({ assets }));
    });
    const fixture = mkdtempSync(join(tmpdir(), 'polaris-release-digest-'));
    const run = promisify(execFile);
    const controller = new AbortController();
    let cleanup: Promise<void> | undefined;
    const dispose = () => {
      if (cleanup) return cleanup;
      controller.abort();
      server.closeAllConnections();
      cleanup = new Promise<void>((resolve, reject) => {
        if (!server.listening) resolve();
        else server.close((error) => error ? reject(error) : resolve());
      }).finally(() => rmSync(fixture, { recursive: true, force: true }));
      return cleanup;
    };
    onTestFinished(dispose);
    try {
      await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
      const address = server.address();
      if (address === null || typeof address === 'string') throw new Error('REST fixture has no TCP port');
      const name = index === 0 ? 'polaris-1.0.0-android-arm64.apk' : 'polaris-1.0.0-linux-x64.AppImage';
      const good = { name, digest: `sha256:${sha}`, state: 'uploaded' };
      const [responseAssets, expectedStatus] = ([
        [[good], 0],
        [[{ name, state: 'uploaded' }], 1],
        [[{ ...good, digest: null }], 1],
        [[{ ...good, digest: `sha256:${'b'.repeat(64)}` }], 1],
        [[{ ...good, state: 'starting' }], 1],
        [[{ name, digest: good.digest }], 1],
        [[{ ...good, name: 'wrong-name.apk' }], 1],
        [[], 1],
        [[good, good], 1],
        [[good, { ...good, name: 'unexpected-asset.zip' }], index === 0 ? 0 : 1],
      ] as const)[caseIndex];
      assets = [...responseAssets];
      const expected = join(fixture, 'expected');
      writeFileSync(expected, `${sha}  ${name}\n`);
      const before = requests;
      // Execute the actual post-upload/read guards, ending before any
      // mutation. Only retry sleeps are elided for the fixed REST fixture.
      const result = await run('bash', ['-c',
        `set -euo pipefail\nsleep() { :; }\n${guard.replace(/^ {10}/gm, '')}`], {
        env: { ...process.env, GH_TOKEN: 'localhost-fixture-not-a-secret',
          GH_CONFIG_DIR: fixture, TMPDIR: fixture, NO_PROXY: '127.0.0.1', no_proxy: '127.0.0.1',
          api_url: `http://127.0.0.1:${address.port}/release`, name, expected,
          expected_sha: sha, android_asset: 'polaris-1.0.0-android-arm64.apk', tag: 'v1.0.0' },
        encoding: 'utf8',
        timeout: 3500,
        killSignal: 'SIGKILL',
        signal: controller.signal,
      }).then(({ stdout, stderr }) => ({ status: 0, stdout, stderr }),
        (error: { code: number; stdout: string; stderr: string }) =>
          ({ status: error.code, stdout: error.stdout, stderr: error.stderr }));
      expect(result.status, `guard ${index}, case ${caseIndex}: ${result.stdout}${result.stderr}`).toBe(expectedStatus);
      expect(requests).toBeGreaterThan(before);
    } finally {
      await dispose();
    }
  });

  it('最终清单包含从草稿回读的 APK，并与远端完整资产集合双向对账后才公开', () => {
    const release = executable(jobBlock(workflow('package.yml'), 'release'));
    const promote = stepBlock(release, 'Promote release to public');
    const downloadAt = release.indexOf('gh release download "$TAG"');
    const sumsAt = release.indexOf('- name: Generate combined SHA256SUMS');
    const uploadSumsAt = release.indexOf('- name: Upload combined SHA256SUMS');
    const verifyAt = release.indexOf('- name: Verify combined asset set and digests');
    const promoteAt = release.indexOf('- name: Promote release to public');
    const remoteTagRefetchAt = release.lastIndexOf('git fetch --force --depth=1 origin');
    const publishCommandAt = release.indexOf(
      'gh release edit "$TAG" --repo "$GITHUB_REPOSITORY" --draft=false',
    );

    expect(downloadAt).toBeGreaterThan(0);
    expect(downloadAt).toBeLessThan(sumsAt);
    expect(sumsAt).toBeLessThan(uploadSumsAt);
    expect(uploadSumsAt).toBeLessThan(verifyAt);
    expect(verifyAt).toBeLessThan(promoteAt);
    expect(remoteTagRefetchAt).toBeGreaterThan(verifyAt);
    expect(remoteTagRefetchAt).toBeLessThan(publishCommandAt);
    expect(release).toContain('android_asset=polaris-${tag#v}-android-arm64.apk');
    expect(promote).toContain(
      'VERIFIED_ANDROID_SHA256: ${{ needs.android_release.outputs.signed_apk_sha256 }}',
    );
    expect(promote).toContain(
      'ANDROID_ASSET: ${{ steps.release_identity.outputs.android_asset }}',
    );
    expect(release).toContain('[ "$downloaded_sha" = "$VERIFIED_ANDROID_SHA256" ]');
    expect(release).toContain('[ "$manifest_rows" != "$VERIFIED_ANDROID_SHA256" ]');
    expect(release).toContain('find . -type f ! -name SHA256SUMS');
    expect(release).toContain("cat \"$sums\"");
    expect(release).toContain(
      `printf '%s  %s\\n' "$(sha256sum "$sums" | cut -d' ' -f1)" 'SHA256SUMS'`,
    );
    expect(release).toContain(
      `pending="$(awk -F'\\t' '$1 == "MISSING" || $2 != "uploaded"' "$raw")"`,
    );
    expect(release).toContain("'SHA256SUMS'");
    expect(release).toContain('diff -u "$expected" "$actual"');
    expect(release.match(/gh api "\$api_url" --jq/g) ?? []).toHaveLength(2);
    expect(release.match(/diff -u "\$expected" "\$actual"/g) ?? []).toHaveLength(2);
    expect(release).toContain('拒绝覆盖 SHA256SUMS');
    expect(release).toContain('公开紧前远端资产已被替换，拒绝公开');
    expect(release).toContain('公开前 tag/release 身份漂移，拒绝继续');
    expect(promote.indexOf('gh api "$api_url" --jq')).toBeLessThan(
      promote.indexOf('diff -u "$expected" "$actual"'),
    );
    expect(promote.indexOf('diff -u "$expected" "$actual"')).toBeLessThan(
      promote.indexOf('gh release edit "$TAG"'),
    );
  });
});
