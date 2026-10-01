#!/usr/bin/env node
/**
 * wiring-verdict —— 「移动端全部接线了没」这句话的**退出码逻辑**，单独成模块因为它必须有测试。
 *
 * # 为什么把判断从 shell 里搬出来
 *
 * 本仓 `gate-node-test.sh` 存在的全部理由就是「判据脚本本身要有测试」，而
 * `scripts/report-wiring.sh` 的退出码逻辑正是这批「全部接线」结论的机器形态，第一版一条测试都没有。
 * 于是它带着两个 bug 交付：
 *  ① **退出码契约只在债务恰好等于基线时成立**：任何一次真正的销账都会让棘轮红 → vitest rc=1 →
 *    脚本落进「门本身红了」那一支，退 2 并把整份 vitest 输出倒进 stderr。而脚本头注把 rc=2
 *    定义为「报告没跑出来」—— 此刻报告跑得好好的，数字也是对的。销账期间每天拿到的都是「门坏了」。
 *  ② **完成路径是个死循环**：债务真的清零那天，脚本叫人去删 `it.fails` 的 `.fails`；
 *    删掉之后它照样这么说，rc 永远是 2，rc=0 那条成功路径到不了。
 *
 * # 判据不靠猜 vitest 的措辞
 *
 * 三个 marker 由两个门自己打印，本模块只读 marker：
 *  · `WIRING_DEBT_TOTAL=<n>`      —— 还欠几条（主门打）
 *  · `WIRING_COMPLETION_GATE=<m>` —— 完成门今天是 `known-red` 还是 `hard`（主门打）
 *  · `HALF_TRUTH_FACTS=ok`        —— 半真事实门真的跑过（姊妹门打）
 * 缺任何一个都退 2：**没执行必须自曝**，不许静默判绿。
 * 「债务漂移」与「别的断言红了」靠棘轮消息里的 `WIRING_DEBT_DRIFT` 这个固定 token 分流，
 * 同样不依赖 vitest 的任何措辞。
 *
 * # 退出码
 *   0  未接线为 0，且两个门都绿 —— 「全部接线」达成
 *   1  还有未接线项（逐条列在报告里），或债务相对基线漂移了（该改基线，不是门坏了）
 *   2  报告没跑出来 / 门本身红了 / 状态自相矛盾 —— 一律不当作通过
 */

export const REPORT_HEAD = '════════ 移动端接线完成度';

/** 从 vitest 输出里抽出报告块（抬头那行到 marker 那行），免得读的人在噪声里找。 */
export function extractReport(output) {
  const lines = output.split('\n');
  const start = lines.findIndex((line) => line.startsWith(REPORT_HEAD));
  if (start < 0) return null;
  const end = lines.findIndex((line, index) => index >= start && /^WIRING_DEBT_TOTAL=\d+$/.test(line));
  if (end < 0) return null;
  return lines.slice(start, end + 1).join('\n');
}

/** vitest 汇总行 `Tests  2 failed | 30 passed (32)` 里的失败条数；解析不出来时返回 null。 */
export function failedCount(output) {
  const summary = /^\s*Tests\s+(.*)$/m.exec(output);
  if (summary === null) return null;
  const failed = /(\d+)\s+failed/.exec(summary[1]);
  return failed === null ? 0 : Number(failed[1]);
}

/**
 * @param {{ output: string, rc: number }} input
 * @returns {{ code: 0|1|2, headline: string, notes: string[], report: string|null, dumpOutput: boolean }}
 */
export function wiringVerdict({ output, rc }) {
  const report = extractReport(output);
  const totalHit = /^WIRING_DEBT_TOTAL=(\d+)$/m.exec(output);
  const modeHit = /^WIRING_COMPLETION_GATE=(known-red|hard)$/m.exec(output);
  const halfTruthRan = /^HALF_TRUTH_FACTS=ok$/m.test(output);
  const failed = failedCount(output);
  const drifted = output.includes('WIRING_DEBT_DRIFT');

  if (totalHit === null) {
    return {
      code: 2,
      headline: "输出里找不到 'WIRING_DEBT_TOTAL=<n>' —— 报告没执行，不当作通过",
      notes: [],
      report,
      dumpOutput: true,
    };
  }
  if (modeHit === null) {
    return {
      code: 2,
      headline: "输出里找不到 'WIRING_COMPLETION_GATE=<known-red|hard>' —— 完成门的强度标记丢了",
      notes: [],
      report,
      dumpOutput: true,
    };
  }
  if (!halfTruthRan) {
    return {
      code: 2,
      headline: "输出里找不到 'HALF_TRUTH_FACTS=ok' —— 半真事实门没跑",
      notes: [
        '这对门缺任何一半都留着一条绕行路：主门守「文案还在」，半真门守「能力今天确实在」。',
        '只跑主门 ⇒ 有人把 dialog 插件注册删掉（导出腿整条没了）时，报告一个字都不会变。',
      ],
      report,
      dumpOutput: true,
    };
  }

  const total = Number(totalHit[1]);
  const mode = modeHit[1];

  // 债务清零那一天：完成门（it.fails）按设计反过来红，棘轮也会因为基线还写着旧清单而红。
  // 这不是门坏了，是完成门在要求把它升格为硬门 —— 这一支必须在通用「门红了」之前分流，
  // 而且必须给出一条**走得到 rc=0** 的指令（第一版那句「去删 .fails」删完还是退 2）。
  if (total === 0 && mode === 'known-red') {
    const upgrade = [
      '两步，缺一不可（改完再跑一次本脚本即 rc=0）：',
      "  ① ui/src/mobile/wiring-completeness.test.ts：COMPLETION_GATE_MODE 改成 'hard'；",
      '  ② 同文件 WIRING_DEBT_BASELINE_IDS 清空成 []。',
    ];
    // rc=0 而债务为 0 意味着完成门**没有**按设计反红 —— 它可能被改成了 it.skip 或被删了。
    // 两种情况的下一步动作一样，但说法必须分开：一种是门在按设计说话，另一种是门不说话了。
    return rc === 0
      ? {
          code: 2,
          headline: '状态自相矛盾：债务为 0，而完成门（known-red）没有反过来红 —— 它可能被改成了 it.skip 或被删了。',
          notes: ['完成门必须两个方向都会说话。确认它还在之后，再走升格：', ...upgrade],
          report,
          dumpOutput: true,
        }
      : {
          code: 2,
          headline: '未接线已经是 0 —— 完成门按设计反过来红了，现在把它升格为硬门。',
          notes: upgrade,
          report,
          dumpOutput: false,
        };
  }

  if (rc !== 0) {
    // 只有棘轮一条红 ⇒ 债务相对基线漂移了。这是**账要改**，不是门坏了：给 rc=1，
    // 别把销账期间的每一天都报成「报告没跑出来」。
    if (drifted && failed === 1) {
      return {
        code: 1,
        headline: `债务清单与基线不一致（当前 ${total} 条）—— 按上面的「新增/消失」逐条改 WIRING_DEBT_BASELINE_IDS。`,
        notes: [
          '消失的条目要在 commit message 里写清是哪条能力接上了、接在哪（file:line）——',
          '不写清就分不清「接上了」和「把文案删了」，而后者是这道门唯一守不住的形态。',
        ],
        report,
        dumpOutput: true,
      };
    }
    return {
      code: 2,
      headline: `门本身红了（vitest rc=${rc}，失败 ${failed ?? '?'} 条）—— 完整输出如下`,
      notes: [],
      report,
      dumpOutput: true,
    };
  }

  if (total > 0) {
    return {
      code: 1,
      headline: `移动端未接线项：${total}（逐条见上）。`,
      notes: [],
      report,
      dumpOutput: false,
    };
  }

  // 到这里必然是 total === 0 且 mode === 'hard' 且 rc === 0：完成门已升格为硬门且真的绿了。
  return {
    code: 0,
    headline: '移动端未接线项：0 —— 「全部接线」达成。',
    notes: [],
    report,
    dumpOutput: false,
  };
}

/* ────────────────────────── CLI ────────────────────────── */

function readStdin() {
  return new Promise((resolveStdin) => {
    let buffer = '';
    process.stdin.setEncoding('utf8');
    process.stdin.on('data', (chunk) => {
      buffer += chunk;
    });
    process.stdin.on('end', () => resolveStdin(buffer));
  });
}

if (process.argv[1] !== undefined && import.meta.url === `file://${process.argv[1]}`) {
  const rcArg = process.argv.find((arg) => arg.startsWith('--rc='));
  const rc = rcArg === undefined ? 0 : Number(rcArg.slice('--rc='.length));
  const output = await readStdin();
  const verdict = wiringVerdict({ output, rc });
  if (verdict.report !== null) process.stdout.write(`${verdict.report}\n`);
  else process.stderr.write('::error::report-wiring: 输出里没有报告块 —— 判据没跑起来，或报告格式变了\n');
  // `::error::` 只留给 rc=2（门坏了 / 没跑起来）。rc=1 是「账上还有待办」，是正常状态，
  // 拿 error 去标它会让 CI 里每一天都像出了事，而真正出事的那天反而不显眼。
  const prefix = { 0: '', 1: '::notice::report-wiring: ', 2: '::error::report-wiring: ' }[verdict.code];
  const sink = verdict.code === 2 ? process.stderr : process.stdout;
  sink.write('\n');
  sink.write(`${prefix}${verdict.headline}\n`);
  for (const note of verdict.notes) sink.write(`        ${note}\n`);
  if (verdict.dumpOutput) process.stderr.write(`${output}\n`);
  process.exit(verdict.code);
}
