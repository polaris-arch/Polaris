/**
 * `wiring-verdict.mjs` 的变异测试。
 *
 * 这份逻辑就是「移动端全部接线了没」这句话的机器形态：它说 rc=0，主会话才能对外说「全部接线」。
 * 本仓的教训是**判据脚本本身要有测试**（`gate-node-test.sh` 存在的全部理由），
 * 而第一版的退出码逻辑一条测试都没有，于是带着两个 bug 交付：
 *  ① 任何一次真正的销账都被报成「门坏了」（rc=2）；
 *  ② 债务清零那天的提示是死循环，rc=0 那条成功路径到不了。
 * 下面每一条都钉住其中一个形态，且**销账 → 清零 → 升格为硬门 → rc=0** 这条完整路径逐步走通。
 */
import { strict as assert } from 'node:assert';
import { test } from 'node:test';

import { extractReport, failedCount, wiringVerdict } from './wiring-verdict.mjs';

/** 造一份长得像 vitest 输出的东西。`extra` 拼在报告块后面。 */
function output({ total = 29, mode = 'known-red', halfTruth = true, failed = 0, extra = '' } = {}) {
  const summary =
    failed > 0 ? `      Tests  ${failed} failed | 30 passed (32)` : '      Tests  31 passed | 1 expected fail (32)';
  return [
    ' RUN  v4.1.11',
    '════════ 移动端接线完成度 ════════',
    '取材面  i18n=27  register=9  control=11  合计=47',
    '处置表  3（逐条见下；判据见 DISPOSITIONS）',
    `未接线  ${total} 条待办 / 21 件工作`,
    `WIRING_COMPLETION_GATE=${mode}`,
    `WIRING_DEBT_TOTAL=${total}`,
    halfTruth ? 'HALF_TRUTH_FACTS=ok' : '',
    extra,
    ' Test Files  2 passed (2)',
    summary,
  ].join('\n');
}

test('报告块抽得出来，且到 marker 那行为止', () => {
  const report = extractReport(output());
  assert.ok(report.startsWith('════════ 移动端接线完成度'));
  assert.ok(report.endsWith('WIRING_DEBT_TOTAL=29'));
  assert.equal(extractReport('什么都没有'), null);
});

test('失败条数解析：没有 failed 段时是 0，解析不出汇总行时是 null', () => {
  assert.equal(failedCount(output()), 0);
  assert.equal(failedCount(output({ failed: 2 })), 2);
  assert.equal(failedCount('没有汇总行'), null);
});

test('还有未接线项 ⇒ rc=1，且不倒完整输出（清单已经在报告块里）', () => {
  const verdict = wiringVerdict({ output: output({ total: 29 }), rc: 0 });
  assert.equal(verdict.code, 1);
  assert.match(verdict.headline, /未接线项：29/);
  assert.equal(verdict.dumpOutput, false);
});

test('marker 缺一不可：三个 marker 各自缺席都退 2（没执行必须自曝）', () => {
  const noTotal = output().replace(/^WIRING_DEBT_TOTAL=\d+$/m, '');
  assert.equal(wiringVerdict({ output: noTotal, rc: 0 }).code, 2);
  const noMode = output().replace(/^WIRING_COMPLETION_GATE=.*$/m, '');
  assert.equal(wiringVerdict({ output: noMode, rc: 0 }).code, 2);
  const noHalfTruth = wiringVerdict({ output: output({ halfTruth: false }), rc: 0 });
  assert.equal(noHalfTruth.code, 2);
  assert.match(noHalfTruth.headline, /HALF_TRUTH_FACTS/);
});

test('半真门没跑 ⇒ 退 2，而不是拿主门的绿冒充「全部接线」', () => {
  // 实测过的形态：删掉 lib.rs 的 dialog 插件注册（导出腿整条没了），半真门当场红，
  // 而只跑主门的报告逐字不变 —— 两扇门之间的缝正好落在唯一那条对外宣布完成的命令上。
  const verdict = wiringVerdict({ output: output({ total: 0, mode: 'hard', halfTruth: false }), rc: 1 });
  assert.equal(verdict.code, 2);
  assert.match(verdict.headline, /半真事实门没跑/);
});

test('销账（债务漂移）⇒ rc=1 并指向基线，不报成「门坏了」', () => {
  // 第一版的 bug：债务从 29 变成 28 时棘轮红 → vitest rc=1 → 落进「门本身红了」→ 退 2，
  // 而头注把 rc=2 定义为「报告没跑出来」。销账期间每天拿到的都是「门坏了」。
  const drifting = output({ total: 28, failed: 1, extra: 'WIRING_DEBT_DRIFT 债务清单与基线不一致。' });
  const verdict = wiringVerdict({ output: drifting, rc: 1 });
  assert.equal(verdict.code, 1);
  assert.match(verdict.headline, /基线不一致（当前 28 条）/);
  assert.match(verdict.notes.join('\n'), /哪条能力接上了/);
});

test('棘轮之外还有别的断言红 ⇒ 仍然退 2（漂移分流不许吞掉真失败）', () => {
  const twoFailures = output({ total: 28, failed: 2, extra: 'WIRING_DEBT_DRIFT 债务清单与基线不一致。' });
  const verdict = wiringVerdict({ output: twoFailures, rc: 1 });
  assert.equal(verdict.code, 2);
  assert.equal(verdict.dumpOutput, true);
});

test('门红了但没有漂移 token ⇒ 退 2 并倒完整输出', () => {
  const broken = output({ total: 29, failed: 1, extra: 'AssertionError: 处置表依据的事实变了' });
  const verdict = wiringVerdict({ output: broken, rc: 1 });
  assert.equal(verdict.code, 2);
  assert.equal(verdict.dumpOutput, true);
});

test('完成路径走得通：清零 → 升格为硬门 → rc=0（第一版这里是死循环）', () => {
  // 第一步：债务清零，完成门（it.fails）按设计反红，棘轮也红（基线还写着旧清单）。
  const zeroKnownRed = output({
    total: 0,
    mode: 'known-red',
    failed: 2,
    extra: 'WIRING_DEBT_DRIFT 债务清单与基线不一致。',
  });
  const step1 = wiringVerdict({ output: zeroKnownRed, rc: 1 });
  assert.equal(step1.code, 2);
  assert.match(step1.headline, /升格为硬门/);
  assert.match(step1.notes.join('\n'), /COMPLETION_GATE_MODE/);
  assert.match(step1.notes.join('\n'), /WIRING_DEBT_BASELINE_IDS/);

  // 第二步：照做之后（mode=hard、基线清空）—— 必须真的到得了 rc=0。
  const step2 = wiringVerdict({ output: output({ total: 0, mode: 'hard' }), rc: 0 });
  assert.equal(step2.code, 0);
  assert.match(step2.headline, /「全部接线」达成/);
});

test('债务为 0、完成门却没有反红 ⇒ 自相矛盾，退 2（挡的是 it.skip 那条路）', () => {
  // known-red 的完成门在债务清零时**必须**反红。它没反红 ⇒ 它不说话了（被 skip 或被删）。
  const verdict = wiringVerdict({ output: output({ total: 0, mode: 'known-red' }), rc: 0 });
  assert.equal(verdict.code, 2);
  assert.match(verdict.headline, /自相矛盾/);
  assert.equal(verdict.dumpOutput, true);
});

test('反向对照：正常的「还欠 29 条」这份输入不许被判成任何一种 2', () => {
  // 「全绿也需反向对照」：上面十条都在证明它报得出错，这一条证明它不是对什么都报错。
  assert.equal(wiringVerdict({ output: output(), rc: 0 }).code, 1);
});
