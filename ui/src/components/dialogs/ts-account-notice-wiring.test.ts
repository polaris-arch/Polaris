/**
 * 「会断开代理」的事前告知是否真的接在五个入口上。纯逻辑（持有判定、只停被确认的那次运行）
 * 由 `ts-logout-flow.test.ts` 守；这里守的是「入口在用它」——函数被测不等于生产在调。
 * 本仓 vitest 无 DOM，交互腿另有两份浏览器测试（需显式开启），故这里取源码级判据：
 * 先剥注释再取材，按函数切片，并自检切片没有切空。
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const read = (rel: string): string =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/.*$/gm, '$1');

/** `from` 起、到其后第一个 `to` 止的切片；任一锚点缺席即红，切片过短也红。 */
function slice(src: string, from: string, to: string): string {
  const start = src.indexOf(from);
  expect(start, `锚点不在了：${from}`).toBeGreaterThanOrEqual(0);
  const end = src.indexOf(to, start + from.length);
  expect(end, `锚点不在了：${to}`).toBeGreaterThan(start);
  const cut = src.slice(start, end);
  expect(cut.length, `切片空了：${from} → ${to}`).toBeGreaterThan(80);
  return cut;
}

const before = (cut: string, first: string, then: string): void => {
  expect(cut, `缺 ${first}`).toContain(first);
  expect(cut, `缺 ${then}`).toContain(then);
  expect(cut.indexOf(first), `${first} 必须先于 ${then}`).toBeLessThan(cut.indexOf(then));
};

describe('account actions say the proxy will be disconnected before it happens', () => {
  it('mobile logout checks the holder and asks again before the backend call', () => {
    const confirm = slice(read('../../mobile/forms/TsSettingsPanel.tsx'),
      "title: t('ts.logout'),", "title={t('ts.settingsTitle')}");
    before(confirm, 'tsNodeHeldByRunningCore(snap, serverId)', 'if (held) {');
    before(confirm, 'if (held) {', "title: t('ts.logoutStopTitle'),");
    before(confirm, "message: t('ts.logoutStopMessage'),", 'await logoutNow();');
    // The stopped-iOS sentence rides on the first confirmation.
    expect(confirm).toContain("'ts.logoutStartsConnectionNote'");
  });

  it('the Android entry marks its platform, or the account switch would stay on the renderer leg', () => {
    const entry = read('../../mobile/MobileMain.tsx');
    before(entry, "platform() === 'android'", "document.documentElement.dataset.mobileOs = 'android'");
    expect(read('./ts-login-server.ts')).toContain("mobileOs === 'ios' || mobileOs === 'android'");
  });

  it('mobile account switch asks before submitting where the backend leaves the proxy stopped', () => {
    const submit = slice(read('../../mobile/forms/TsLoginPanel.tsx'),
      'const submit = async (stopNoticed = false)', 'await executeTsLogin(');
    before(submit, 'backendReplacement && !isIOS && !stopNoticed', 'tsNodeHeldByRunningCore(snap, heldId)');
    before(submit, "title: t('ts.switchStopTitle'),", 'await submit(true);');
    before(submit, 'await submit(true);', 'tsLoginUsesBackendCredentials(submissionBase');
  });

  it('desktop account switch offers to stop the holding run and submits again', () => {
    const src = read('./TsLoginDialog.tsx');
    const ask = slice(src, 'const askStopThenRetry = async', 'const handleSubmit = async');
    before(ask, 'captureMainCoreOwner(id,', "title: t('ts.switchStopTitle'),");
    before(ask, 'await stopOwnedCore(owner,', "if (stopped === 'stopped') await submitRef.current();");
    const submit = slice(src, 'const handleSubmit = async', 'const copyAuthUrl');
    before(submit, "outcome.reason === 'mainCoreInUse'", 'void askStopThenRetry(server.id)');
    expect(submit).toContain('submitRef.current = handleSubmit;');
  });

  it('both desktop logout entries hand the main-core refusal to the shared confirmation', () => {
    const settings = slice(read('./TsSettingsDialog.tsx'), 'const handleLogout = async', 'const authKeyRow');
    before(settings, 'if (isMainCoreLogoutError(e)) {', 'await confirmStopThenTsLogout(node.id, t, loggedOut);');
    const nodes = slice(read('../screens/nodes/NodesScreen.tsx'), 'const tsLogout = useCallback(', 'const nodeDeletion');
    before(nodes, 'isMainCoreLogoutError(result.error)', 'await confirmStopThenTsLogout(node.id, t,');
    const shared = read('./ts-logout-confirm.ts');
    before(shared, 'captureMainCoreOwner(serverId,', "title: t('ts.logoutStopTitle'),");
    before(shared, "message: t('ts.logoutStopMessage'),", 'await stopOwnedCoreThenLogout(owner,');
  });
});
