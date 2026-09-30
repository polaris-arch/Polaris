/**
 * 移动端**锁屏遮罩**（W-16）。桌面对照：`components/layout/LockOverlay.tsx`。
 *
 * # 为什么不 import 桌面那一个
 *
 * 它的外观全部落在 `#lock-overlay` / `.lock-box` / `.input` / `.btn flow` 这些选择器上，而那些规则
 * 住在 `prototype.css` / `components.css` —— 即契约 A1 禁止进入移动入口的桌面层叠链
 * （`mobile-entry.test.ts` 会当场红）。**逻辑一个字不重写**：解锁链在 `privacy-lock.ts`，
 * 空密码放行判定在 `@/domain/privacy` 的 `resolveUnlockAttempt`，两端读同一份。
 * 样式内联，理由与 `settings/SettingsChrome.tsx` 头注同一条（本屏不新增 `.css`，A1 白名单不动）。
 *
 * # 🔴 三条不变量，每条都对应一个真实的失效形态
 *
 * 1. **不登记进 `back-stack`。** 那张表是「系统返回键要按 LIFO 关掉的可关闭层」；把遮罩登记进去
 *    ⇒ 按一下返回就把它 `dismiss` 掉 = **返回键成了解锁键**，密码形同虚设。故本组件不调
 *    `useDismissableLayer`，而 `MobileApp.handleBackPress` 在锁定态**第一件事**就是早退到
 *    「交还系统」那一档（连 `dismissTop()` 都不调，免得顺手弹掉底下某个真弹层）。
 * 2. **盖住全部内容。** `position:fixed` + 四边 `inset:0` + 不透明实底 + `zIndex` 高于停靠区。
 *    渲染在 `MobileShell` **之外**（`MobileApp` 的兄弟节点）：外壳内部任何一层的 `z-index`
 *    都在它自己的层叠上下文里，压不过一个 fixed 的兄弟。
 * 3. **`locked` 为假时返 `null`。** 不是 `display:none` —— DOM 里留着一个能拿到焦点的密码框
 *    是另一回事。
 *
 * # 🔴 锁定态是 **prop**，不是本组件自己读 store
 *
 * 两个理由，第二个是承重的：
 *  · `MobileApp` 本来就要读它（返回键那一档），两处各读一次等于同一个字段有两个读点；
 *  · **判据驱动得动它**。本仓 vitest 跑 node、判据面是 `renderToStaticMarkup`，而 zustand v5 的
 *    `useStore` 把 `getInitialState` 当 server snapshot（`MobileShell.tsx` 头注记过同一个坑）——
 *    自己读 store 的话，判据无论怎么写 store，渲染出来的永远是**建 store 那一刻**的值，
 *    于是「遮罩到底出不出」这件事**没有任何门守得住**。收成 prop 之后，「遮罩在锁定态下画不画、
 *    画出来盖不盖得住、登不登记进 back-stack」三条全部变成可驱动的行为断言；
 *    而「`MobileApp` 真的把那个值喂了进来」由一条 AST 断言单独钉（判的是那个 JSX 属性的**表达式**，
 *    不是「属性名出现过」—— 恒传 `false` 的容器正是那条断言要抓的形态）。
 *
 * # 状态机（四态，全在本组件的三个 `useState` 里）
 *
 * ```
 *              privacyMode:true（后端 emit enterPrivacyMode → app-wiring 收敛进 store）
 *   未锁 ─────────────────────────────────────────────────────────────────────→ 锁定/待输入
 *                                                                                  │ 提交
 *                                                                                  ▼
 *                                          require-input ──────────────────→ 锁定/提示补密码
 *   未锁 ←──── privacyMode:false（后端 emit exitPrivacyMode）←── unlocked ←── 锁定/提交中
 *                                          rejected / failed ───────────────→ 锁定/显示原因
 * ```
 * 「解锁成功 ⇒ 遮罩消失」这一跳**刻意不在本组件里写**：它由 store 的 `privacyMode` 变假驱动，
 * 而那个值的唯一来源是后端事件（见 `privacy-lock.ts` 头注「为什么翻转走 `config_set_privacy_mode`」）。
 * 在这里手动 `setLocked(false)` 会得到一个「界面解锁了、后端还锁着」的分叉态。
 */

import { useEffect, useRef, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';

import {
  readPrivacyHasPassword,
  submitPrivacyUnlock,
  unlockOutcomeMessageKey,
  type UnlockOutcome,
} from './privacy-lock';

/**
 * 遮罩的层号。比停靠区里任何一件 chrome 都高一档；数值本身不重要，重要的是**它是 fixed 兄弟节点**
 * （见头注不变量 2）—— 那才是「压得住」的结构性理由，`zIndex` 只是同层内的保险。
 */
const OVERLAY_Z = 90;

export function MobileLockOverlay({ locked }: { locked: boolean }): ReactElement | null {
  const { t } = useTranslation();
  const [password, setPassword] = useState('');
  const [hasPassword, setHasPassword] = useState(true);
  const [outcome, setOutcome] = useState<UnlockOutcome | null>(null);
  const [busy, setBusy] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  /* 进入锁定态：复位输入与提示，并查一次「设没设密码」（决定空密码放不放行）。
     退出锁定态同样复位 —— 否则下一次锁上时，上一次那行「密码错」还挂着。 */
  useEffect(() => {
    setPassword('');
    setOutcome(null);
    if (!locked) return;
    void readPrivacyHasPassword().then(setHasPassword);
  }, [locked]);

  if (!locked) return null;

  const submit = (): void => {
    if (busy) return;
    setBusy(true);
    void submitPrivacyUnlock(hasPassword, password, (next) => {
      setOutcome(next);
      setBusy(false);
      /* 密码错就清空输入框（同桌面）：留着错的那串，下一次多半是在它后面继续打。 */
      if (next === 'rejected') setPassword('');
    });
  };

  const messageKey = outcome === null ? null : unlockOutcomeMessageKey(outcome);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={t('privacy.title')}
      data-overlay="privacy-lock"
      style={{
        position: 'fixed',
        top: 0,
        right: 0,
        bottom: 0,
        left: 0,
        zIndex: OVERLAY_Z,
        /* 不透明实底。半透明 + blur 在这里是**安全**问题不是观感问题：底下那一屏正是要挡住的东西。 */
        background: 'hsl(var(--bg))',
      }}
    >
      <div className="m-lock-viewport">
      <div className="m-lock-content">
      <h2
        style={{
          margin: 0,
          fontFamily: 'var(--disp)',
          fontSize: '1.3125rem',
          fontWeight: 600,
          color: 'hsl(var(--fg))',
        }}
      >
        {t('privacy.title')}
      </h2>
      <p style={{ margin: 0, fontSize: '0.875rem', lineHeight: 1.5, color: 'hsl(var(--fg-dim))' }}>
        {t('privacy.subtitle')}
      </p>
      <div style={{ display: 'flex', gap: '8px', width: '100%', maxWidth: '320px' }}>
        <input
          ref={inputRef}
          type="password"
          data-lock-input="privacy"
          aria-label={t('privacy.title')}
          autoComplete="off"
          value={password}
          disabled={busy}
          onChange={(e) => setPassword(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') submit();
          }}
          style={{
            flex: '1 1 auto',
            minWidth: 0,
            minHeight: 'var(--tap-min)',
            boxSizing: 'border-box',
            padding: '8px 10px',
            border: '1px solid hsl(var(--line))',
            borderRadius: 'var(--r-sm)',
            background: 'hsl(var(--surface-2))',
            color: 'hsl(var(--fg))',
            fontFamily: 'inherit',
            fontSize: '0.875rem',
          }}
        />
        <button
          type="button"
          data-lock-submit="privacy"
          disabled={busy}
          onClick={submit}
          style={{
            flex: '0 0 auto',
            minHeight: 'var(--tap-min)',
            padding: '0 14px',
            border: '1px solid hsl(var(--flow))',
            borderRadius: 'var(--r-sm)',
            background: 'hsl(var(--flow))',
            color: '#00141d',
            fontFamily: 'inherit',
            fontSize: '0.875rem',
            fontWeight: 500,
            opacity: busy ? 0.5 : 1,
            cursor: busy ? 'default' : 'pointer',
            touchAction: 'manipulation',
          }}
        >
          {t('privacy.unlock')}
        </button>
      </div>
      {/* 提示行**恒在**（占位高度固定）：出现/消失会让上面那一排控件跳一下，而这一跳恰好发生在
          用户刚提交完、视线正盯着输入框的时候。 */}
      <span
        role="alert"
        data-lock-message="privacy"
        style={{ minHeight: '18px', fontSize: '0.75rem', color: 'hsl(var(--err))' }}
      >
        {messageKey === null ? '' : t(messageKey)}
      </span>
      </div>
      </div>
    </div>
  );
}
