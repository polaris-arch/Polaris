/**
 * 移动端应用分流策略选择器的 **UI 侧判据**（跨语言缝的这一端）。
 *
 * # 这道门补的是哪条缝
 *
 * `crates/config-engine/tests/golden_inbounds_android.rs` 已经钉住「`direct` 档的包名进
 * `tun.exclude_package`、另三档不进」。但那条测的输入是**本仓自撰**的 Rust 侧夹具 ——
 * 它证明「引擎认这种形态」，证明不了「UI 写出来的就是这种形态」。UI 那边把 `enabled` 写成
 * `false`、把「跟随全局」写成「删掉这条规则」、或者把 `action` 拼错一个字母，Rust 侧一个字都不会红，
 * 而用户在手机上点「直连」什么都不会发生。**缝正好落在两者之间。**
 *
 * 故两端读**同一份**夹具 `crates/config-engine/fixtures/mobile-app-policy-writes.json`：
 *  · 本文件断言 `appRuleForPick` 逐字段产出夹具里那两条可写形态，且 `APP_POLICY_WRITABLE`
 *    恰好是夹具标了 `writable` 的那两档；
 *  · Rust 侧 `mobile_policy_picks_reach_exclude_package_exactly_for_direct` 拿同一批 `rule`
 *    当输入跑 `build_inbounds`。
 *
 * # 「测了纯函数 ≠ 生产在用它」——两条判据成对交
 *
 * 抽出 `appRuleForPick` 这个动作**自己造了一条新缝**：函数被测得好好的，而容器里另拼一个对象。
 * 故本文件第二组是**接线**判据：`RulesScreen.tsx` 的写路径必须真的调用它，并且把它的返回值
 * 同时交给暂存腿（`nextValue`）与直落盘腿（`mutateConfigEntities` 的 `value`）；
 * 少交一条腿 = 那条腿上写的是另一种形态。每一条都带**反向对照**（把源码改成绕过工厂函数，
 * 谓词必须转 false），否则「恒 true 的谓词」与「接线正确」在这里长得一模一样。
 *
 * # 射程自曝
 *
 *  · 接线那一组是**词法**判据：它读的是 `RulesScreen.tsx` 的源码文本，不是运行期行为
 *    （本仓 vitest 跑在 node 环境，没有 DOM，点不动那颗触发器）。
 *
 *    🔴 **2026-09-06 更正一句说错的射程自曝**：上一版写的是「抓不到『把整个
 *    `handleAppPolicyPick` 从 JSX 上摘下来』——那一格由 `rules-screen.test.tsx` 的渲染面兜」。
 *    **那个兜底不存在**：`rules-screen.test.tsx:257` 的 `appsMarkup` 直接渲染 `AppsSegment`
 *    并把 `onPolicyPick` 打成 `() => {}` 桩，一个字都没读过容器那段 JSX。实测把容器改成
 *    `onPolicyPick={(appId) => void handleAppPolicyPick(appId, 'follow')}`（丢掉用户选的档位）：
 *    `tsc` / 全量 `vitest` / `gate-node-test.sh` / `report-wiring.sh` **一条不红**，而用户点「直连」
 *    落库的是「跟随全局」。⇒ 现在那一格由本文件的**③ 容器 → 分段**那一组自己收：
 *    切出 `<AppsSegment` 那段 props，断言 `onPolicyPick` 逐字透传 `(appId, pick)` 两个形参。
 *  · 它不证明真机上 `addDisallowedApplication` 真的把应用踢出了隧道 —— 那归真机验收。
 *    本门与 Rust 侧那条合起来只证明到「配置里那个字段是对的」。
 */
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import {
  APP_POLICY_WRITABLE,
  appRuleForPick,
  isAppPolicyWritable,
  type AppPolicyWritablePick,
} from './app-policy-write';

const HERE = fileURLToPath(new URL('.', import.meta.url));
/** 仓根（本文件在 `ui/src/mobile/screens/rules/`，往上五层）。 */
const REPO = fileURLToPath(new URL('../../../../../', import.meta.url));
const FIXTURE_REL = 'crates/config-engine/fixtures/mobile-app-policy-writes.json';
const GOLDEN_REL = 'crates/config-engine/tests/golden_inbounds_android.rs';

interface PickEntry {
  pick: string;
  writable: boolean;
  /* `targetServerId` 单列出来是因为「指定节点」那一档要把它原样喂回工厂函数 ——
     从 `Record<string, unknown>` 里取会得到 `unknown`，而喂错类型就测不出那一档。 */
  rule: Record<string, unknown> & { targetServerId?: string };
  expectExcluded: boolean;
}
interface PickFixture {
  appId: string;
  picks: PickEntry[];
  expectedPackage: string;
  baseConfig: Record<string, unknown>;
}

const FIXTURE = JSON.parse(readFileSync(REPO + FIXTURE_REL, 'utf8')) as PickFixture;

/** 去掉块注释与行注释 —— 注释里写着的反面例子不是代码（本仓被这么喂绿过）。 */
function stripComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .split('\n')
    .map((line) => line.replace(/(^|[^:])\/\/.*$/, '$1'))
    .join('\n');
}

/**
 * 切出容器里那一段写路径（`handleAppPolicyPick` 的函数体）。
 *
 * 切片而不是全文匹配：全文里别处也有 `mutateConfigEntities`（资源删除那条腿），
 * 拿全文判「有没有调用」会被邻居喂绿。切不出来要当**红**报，不能静默退回全文。
 */
function policyWriteSlice(source: string): string {
  const text = stripComments(source);
  const from = text.indexOf('const handleAppPolicyPick');
  const to = text.indexOf('const [resSource', from);
  if (from < 0 || to < 0 || to <= from) {
    throw new Error(
      '切不出 `handleAppPolicyPick` 的写路径切片 —— 容器结构变了。' +
        '不许静默退回全文匹配：那会让隔壁那条 `mutateConfigEntities` 把本门喂绿。',
    );
  }
  return text.slice(from, to);
}

/**
 * 接线的四个事实。每一条都要有反向对照，否则「恒 true」与「接对了」不可分。
 *
 * 🔴 入参是**切片**，不是整份源码。第一版收整份、自己在里面切，于是反向对照只能对整份做
 * `String.replace` —— 而 `nextValue: next` 在这份容器里出现**两次**（`handleRegionChange`
 * 那条暂存腿逐字相同），`replace` 打在了那一处，被测的这段一个字没改，对照当场失效。
 * 实测：改完之后谓词照样报 true。切片先行之后，变异与判据落在同一段文本上。
 */
function wiringFacts(slice: string): {
  callsFactory: boolean;
  stagesFactoryResult: boolean;
  mutatesFactoryResult: boolean;
  buildsItsOwnRule: boolean;
} {
  return {
    /* 第三个实参（指定节点那一档的 `targetServerId`）是**可选**的：谓词认两参与三参两种形态，
       但仍然钉死前两个实参必须逐字是 `appId, pick` —— 换成别的变量就不是这条写路径了。 */
    callsFactory:
      /\bconst\s+next\s*:\s*AppRule\s*=\s*appRuleForPick\(\s*appId\s*,\s*pick\s*(?:,\s*[A-Za-z_$][\w$]*\s*)?\)/.test(
        slice,
      ),
    stagesFactoryResult: /\bnextValue:\s*next\b/.test(slice),
    mutatesFactoryResult:
      /mutateConfigEntities\(\s*\[\s*\{\s*collection:\s*'appRules',\s*entityId:\s*appId,\s*value:\s*next\s*\}/.test(
        slice.replace(/\s+/g, ' '),
      ),
    /*
     * 切片里再出现一个 `action` 字段 = 有人在这里又拼了一份形态。
     * **不限定后面跟引号**：第一版写的是 `action:\s*['"`]`，而绕过工厂函数最自然的写法
     * （`action: pick === 'direct' ? 'direct' : 'proxy'`）后面跟的是标识符，整条溜过去。
     */
    buildsItsOwnRule: /\baction\s*:/.test(slice),
  };
}

/**
 * 切出容器把选择结果交给分段的那一跳（`<AppsSegment ... />` 的 props 段）。
 *
 * 与写路径切片同一个理由（见 [`policyWriteSlice`]）：全文里 `onPolicyPick` 还出现在
 * `AppsSegment.tsx` 的类型定义与面板那一侧，拿全文判会被邻居喂绿。切不出来当**红**报。
 */
function segmentPropsSlice(source: string): string {
  const text = stripComments(source);
  const from = text.indexOf('<AppsSegment');
  const to = text.indexOf('/>', from);
  if (from < 0 || to < 0 || to <= from) {
    throw new Error(
      '切不出 `<AppsSegment …/>` 那段 props —— 容器结构变了。' +
        '不许静默退回全文匹配：`onPolicyPick` 在别处还出现，全文判会被邻居喂绿。',
    );
  }
  return text.slice(from, to);
}

const RULES_SCREEN = readFileSync(HERE + 'RulesScreen.tsx', 'utf8');
const APPS_SEGMENT = readFileSync(HERE + 'AppsSegment.tsx', 'utf8');
/** 被判的那一段。变异也打在它身上 —— 见 [`wiringFacts`] 头注那条实测。 */
const SLICE = policyWriteSlice(RULES_SCREEN);
/** 容器 → 分段那一跳。 */
const SEGMENT_PROPS = segmentPropsSlice(RULES_SCREEN);

/** 在切片上做一处定点变异，并**证明它打上了**（没命中的变异等于什么都没测）。 */
function mutateSlice(from: string, to: string): string {
  expect(SLICE.includes(from), `变异切点「${from}」不在写路径切片里 —— 对照测的是空气`).toBe(true);
  const next = SLICE.replace(from, to);
  expect(next).not.toBe(SLICE);
  return next;
}

/**
 * 容器那一跳的 prop 原文（变异切点）。**逐字**取自生产源码里那一行 —— 换行位置一起取，
 * 因为 prettier 无关的换行会让 `String.includes` 落空，而落空的变异「测的是空气」。
 */
const ONPICK_PROP =
  'onPolicyPick={(appId, pick, targetServerId) =>\n            void handleAppPolicyPick(appId, pick, targetServerId)\n          }';

/** 同上，打在容器 → 分段那段 props 上。 */
function mutateSegmentProps(from: string, to: string): string {
  expect(
    SEGMENT_PROPS.includes(from),
    `变异切点「${from}」不在 <AppsSegment> 那段 props 里 —— 对照测的是空气`,
  ).toBe(true);
  const next = SEGMENT_PROPS.replace(from, to);
  expect(next).not.toBe(SEGMENT_PROPS);
  return next;
}

/**
 * 容器 → 分段这一跳的**唯一**事实：用户选的那一档逐字透传，不被容器换成字面量。
 *
 * 三个形参都要判：只判 `handleAppPolicyPick(appId, pick, …)` 会被
 * `onPolicyPick={(appId) => void handleAppPolicyPick(appId, pick)}` 这种（`pick` 从外层作用域
 * 捞一个同名变量）绕过；只判箭头形参会被丢掉实参绕过。
 *
 * 🔴 第三个形参（`targetServerId`）2026-09-13 随「指定节点」那一档接通而加。它**同样要逐字透传**：
 * 丢掉它，选中一个节点会落成「跟随全局」—— pill 变色、面板收起，落库的却是另一档，
 * 那正是本组存在的那条静默变异，只是换了一格。
 */
function passesUserPick(slice: string): boolean {
  return /onPolicyPick=\{\(\s*appId\s*,\s*pick\s*,\s*targetServerId\s*\)\s*=>\s*\n?\s*void\s+handleAppPolicyPick\(\s*appId\s*,\s*pick\s*,\s*targetServerId\s*\)\s*\}/.test(
    slice,
  );
}

describe('移动端策略选择器：写出来的形态 ←→ 引擎认的形态', () => {
  describe('⓪ 自检：夹具与取材面都是活的', () => {
    it('夹具四档齐、可写的恰好两档、且恰好一档期望进 exclude_package', () => {
      expect(FIXTURE.picks.length, '夹具不是四档 —— 有一档没被判过').toBe(4);
      /* 2026-09-13（批 10）：四档全部可写。`expectExcluded` 仍恰好一档 —— 那是**引擎**侧的事实，
         与「UI 写不写得出」正交，它不该跟着一起变（两者一起变过一次就说明判据被并成了一条）。 */
      expect(FIXTURE.picks.filter((p) => p.writable).map((p) => p.pick)).toEqual([
        'follow',
        'direct',
        'proxy',
        'block',
      ]);
      expect(FIXTURE.picks.filter((p) => p.expectExcluded).length).toBe(1);
      expect(FIXTURE.expectedPackage, '期望包名为空 ⇒ 正反两面会一起退化成空表').toBeTruthy();
    });

    it('Rust 侧读的是**同一份**夹具（两端各读一份就等于两个真值源）', () => {
      const golden = readFileSync(REPO + GOLDEN_REL, 'utf8');
      expect(
        golden,
        'golden_inbounds_android.rs 不再读这份夹具 —— 缝的另一端断了，本门单独绿没有意义',
      ).toContain('mobile-app-policy-writes.json');
      expect(golden).toContain('fn mobile_policy_picks_reach_exclude_package_exactly_for_direct');
      // `appRoutingEnabled` 缺省=开（与本端 `!== false` 同口径）的引擎侧判据也必须还钉着。
      expect(golden).toContain('fn app_routing_gate_absent_key_means_on');
    });

    it('容器源码切得出写路径，且切到的是真东西', () => {
      expect(SLICE.length, '切片太短 —— 大概率切歪了').toBeGreaterThan(200);
      expect(SLICE).toContain('appRules');
      // 切片必须**真的把邻居挡在外面**：`nextValue: next` 在整份容器里出现不止一次
      //（`handleRegionChange` 那条暂存腿逐字相同），全文匹配会被邻居喂绿。
      expect(
        (RULES_SCREEN.match(/nextValue: next,/g) ?? []).length,
        '整份容器里只剩一处 `nextValue: next` ⇒ 上面那条「切片而非全文」的理由要重新核',
      ).toBeGreaterThan(1);
      expect((SLICE.match(/nextValue: next,/g) ?? []).length).toBe(1);
    });
  });

  describe('① 纯函数：`appRuleForPick` 与夹具逐字段一致', () => {
    for (const entry of FIXTURE.picks.filter((p) => p.writable)) {
      it(`「${entry.pick}」档落成的 AppRule 与夹具逐字段相同`, () => {
        /* 「指定节点」那一档的落盘形态里带着节点 id，故把夹具里那一格原样喂回去 ——
           不喂的话它会落成「跟随全局」，而那正是这条断言要分辨的两种形态。 */
        const got = appRuleForPick(
          FIXTURE.appId,
          entry.pick as AppPolicyWritablePick,
          entry.rule.targetServerId,
        );
        // 深比较**双向**：`toEqual` 对 `undefined` 字段是宽松的，故再比一次 JSON 键序无关的形状，
        // 把「多写了一个 `targetServerId: undefined`」这种会让序列化结果分叉的写法也挡住。
        expect(got).toEqual(entry.rule);
        expect(Object.keys(got).sort()).toEqual(Object.keys(entry.rule).sort());
        expect(JSON.parse(JSON.stringify(got))).toEqual(entry.rule);
      });
    }

    it('四档写出来的确实是**四种**形态（否则上面几条可以同时被一个常量满足）', () => {
      expect(appRuleForPick('x', 'direct')).not.toEqual(appRuleForPick('x', 'follow'));
      expect(appRuleForPick('x', 'block')).not.toEqual(appRuleForPick('x', 'direct'));
      expect(appRuleForPick('x', 'proxy', 'srv-1')).not.toEqual(appRuleForPick('x', 'follow'));
      expect(appRuleForPick('x', 'direct').action).toBe('direct');
      expect(appRuleForPick('x', 'block').action).toBe('block');
      expect(appRuleForPick('x', 'follow').action).toBe('proxy');
      expect(appRuleForPick('x', 'proxy', 'srv-1').targetServerId).toBe('srv-1');
      /* 🔴 「代理但没给节点」= 跟随全局，不是一条半成品规则。写成别的会让「取消指定节点」
         这个动作落不出可提交的形态。 */
      expect(appRuleForPick('x', 'proxy')).toEqual(appRuleForPick('x', 'follow'));
      /* 且**不许**多写一个 `targetServerId: undefined`：那会让序列化结果与夹具分叉。 */
      expect(Object.keys(appRuleForPick('x', 'proxy')).sort()).toEqual(
        ['action', 'appId', 'enabled'].sort(),
      );
      // `enabled` 恒 true 是刻意的（移动端没有逐条启停的控件）：写成 false 会被引擎整条滤掉。
      expect(appRuleForPick('x', 'direct').enabled).toBe(true);
      expect(appRuleForPick('x', 'follow').enabled).toBe(true);
      // appId 原样透传，不做任何归一（引擎按它查预设包名）。
      expect(appRuleForPick('custom-1', 'direct').appId).toBe('custom-1');
    });

    it('白名单与夹具的 `writable` 一致，且真的挡得住另两档', () => {
      expect([...APP_POLICY_WRITABLE].sort()).toEqual(
        FIXTURE.picks
          .filter((p) => p.writable)
          .map((p) => p.pick)
          .sort(),
      );
      // 正反各证一次：只写「挡住了 block」会被「它对谁都返回 false」骗过。
      /* 四档全放行之后这条谓词守的不再是「哪几档可写」，而是「档位串本身合法」——
         正反各证一次：只证四档为真会被「它对谁都返回 true」骗过。 */
      expect(isAppPolicyWritable('direct')).toBe(true);
      expect(isAppPolicyWritable('follow')).toBe(true);
      expect(isAppPolicyWritable('proxy')).toBe(true);
      expect(isAppPolicyWritable('block')).toBe(true);
      expect(isAppPolicyWritable('')).toBe(false);
      expect(isAppPolicyWritable('bypass'), '未知档位串被放行 ⇒ 会落成一条 action 为空的规则').toBe(
        false,
      );
    });
  });

  describe('② 接线：生产写路径真的用它（测了纯函数 ≠ 生产在用它）', () => {
    it('两条腿都交的是工厂函数的返回值，且容器里没有第二份形态', () => {
      const facts = wiringFacts(SLICE);
      expect(facts.callsFactory, '写路径没有调用 `appRuleForPick(appId, pick)`').toBe(true);
      expect(facts.stagesFactoryResult, '暂存腿交的不是工厂函数的返回值').toBe(true);
      expect(facts.mutatesFactoryResult, '直落盘腿交的不是工厂函数的返回值').toBe(true);
      expect(
        facts.buildsItsOwnRule,
        '容器里又出现了一个 `action:` 字面量 —— 形态有了第二份真值',
      ).toBe(false);
    });

    it('反向对照：把工厂函数换成就地拼一个对象，两条事实必须跟着翻', () => {
      // 变异体**刻意**用三元而不是裸字面量：绕过工厂函数最自然的写法就是它，
      // 而第一版的谓词只认 `action: '…'`，整条从它下面溜过去（实测报 true）。
      const facts = wiringFacts(
        mutateSlice(
          'const next: AppRule = appRuleForPick(appId, pick, targetServerId);',
          "const next: AppRule = { appId, action: pick === 'direct' ? 'direct' : 'proxy', enabled: true };",
        ),
      );
      expect(facts.callsFactory).toBe(false);
      expect(facts.buildsItsOwnRule).toBe(true);
    });

    it('反向对照：把暂存腿的 `nextValue` 改成别的值，谓词必须转 false', () => {
      expect(
        wiringFacts(mutateSlice('nextValue: next,', 'nextValue: { appId },')).stagesFactoryResult,
      ).toBe(false);
    });

    it('反向对照：把直落盘腿的 `value` 改成别的值，谓词必须转 false', () => {
      expect(
        wiringFacts(
          mutateSlice(
            "{ collection: 'appRules', entityId: appId, value: next }",
            "{ collection: 'appRules', entityId: appId, value: { appId } }",
          ),
        ).mutatesFactoryResult,
      ).toBe(false);
    });

    it('暂存闸门在这条腿上（`appRules` 是 Class B，少了它开着暂存也会直接落盘）', () => {
      expect(SLICE, "写路径少了 `editRoute('appRules', …)` 分流").toMatch(
        /editRoute\(\s*'appRules'\s*,\s*stagingEnabled\s*\)\s*===\s*'staged'/,
      );
    });

    it('面板那一侧也收窄（UI 拦住 ≠ 写路径拦住，两条判据要成对交）', () => {
      const segment = stripComments(APPS_SEGMENT);
      /*
       * 白名单那一层仍在，只是守的东西变了（2026-09-13 四档全开）：它不再挡「哪几档可写」，
       * 而是挡**档位串本身不合法**的调用。判据跟着改成「走了白名单」而不是「有两档 disabled」。
       */
      expect(segment, '面板的 onSelect 没有过白名单').toContain('isAppPolicyWritable(value)');
      /* 节点那一档走的是另一条分支（`node:` 前缀），它不过白名单 —— 白名单只认四个档位串。
         这条断言钉住那条分支真的在：少了它，选中一个节点会掉进白名单被整条丢掉。 */
      expect(segment, '「指定节点」那条分支没了 —— 选中节点会被白名单丢掉').toContain(
        "value.startsWith('node:')",
      );
      /* 反面：面板里**不许**再有写死禁用的档位（那正是本批撤销的形态）。 */
      expect(segment).not.toMatch(/id:\s*'proxy'[^\n]*disabled:\s*true/);
      expect(segment).not.toMatch(/id:\s*'block'[^\n]*disabled:\s*true/);
    });
  });

  describe('③ 容器 → 分段：用户选的那一档真的走到写路径（②的射程只到函数体内）', () => {
    it('自检：切得出 `<AppsSegment …/>` 那段 props，且切到的是真东西', () => {
      expect(SEGMENT_PROPS.length, '切片太短 —— 大概率切歪了').toBeGreaterThan(200);
      expect(SEGMENT_PROPS).toContain('onPolicyPick=');
      // 切片必须真的把邻居挡在外面：`onPolicyPick` 在整份仓里还出现在分段自己的类型与面板那一侧。
      expect(SEGMENT_PROPS).not.toContain('<ResourcesSegment');
      expect(SEGMENT_PROPS).not.toContain('handleAppPolicyPick =');
    });

    it('🔴 `onPolicyPick` 逐字透传 `(appId, pick)` —— 容器不许把用户选的档位换成字面量', () => {
      expect(
        passesUserPick(SEGMENT_PROPS),
        '容器没有把用户选的那一档交给写路径 —— 点「直连」落库的会是另一档，' +
          '而 ② 那一组（只读 `handleAppPolicyPick` 函数体）对此一个字都不会红',
      ).toBe(true);
    });

    it('反向对照：把 `pick` 换成字面量，谓词必须转 false', () => {
      // 这正是实测过的那条静默变异：pill 变色、面板收起，落库的却是「跟随全局」。
      expect(
        passesUserPick(
          mutateSegmentProps(
            ONPICK_PROP,
            "onPolicyPick={(appId) => void handleAppPolicyPick(appId, 'follow')}",
          ),
        ),
      ).toBe(false);
    });

    it('反向对照：把这条 prop 整个摘掉，谓词必须转 false', () => {
      expect(
        passesUserPick(
          mutateSegmentProps(
            ONPICK_PROP,
            'onPolicyPick={() => {}}',
          ),
        ),
      ).toBe(false);
    });

    it('分段那一侧同样逐字透传（同一条缝在 `handlePick` 上也开着）', () => {
      const segment = stripComments(APPS_SEGMENT);
      /*
       * 面板 `onSelect` 收到的是一个**值编码**，分段把它解回「哪一档 + 哪个节点」再交给容器。
       * 两条出口都要判：节点那条必须把 id 带上，档位那条必须把 `value` 原样交出去。
       * 只判一条会被另一条的字面量绕过（那正是本组头注那条静默变异的形状）。
       */
      expect(
        /onPolicyPick\(\s*appId\s*,\s*'proxy'\s*,\s*value\.slice\(5\)\s*\)/.test(segment),
        '选中一个节点时没有把节点 id 交出去 —— 会落成「跟随全局」',
      ).toBe(true);
      expect(
        /onPolicyPick\(\s*appId\s*,\s*value\s*\)/.test(segment),
        '选中一个档位时交出去的不是用户点的那一档',
      ).toBe(true);
      // 反向对照：换成字面量后谓词必须转 false（否则它只是「这串文本恒在」）。
      const bypassed = segment.replace(
        /onPolicyPick\(\s*appId\s*,\s*value\s*\)/,
        "onPolicyPick(appId, 'follow')",
      );
      expect(bypassed).not.toBe(segment);
      expect(/onPolicyPick\(\s*appId\s*,\s*value\s*\)/.test(bypassed)).toBe(false);
    });
  });
});
