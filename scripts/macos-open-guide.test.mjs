import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { macOpenGuideViolations } from './lib/macos-open-guide.mjs';
const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const guide = read('packaging/macos-dmg-open-guide.txt');
const names = ['README.md', 'README.en.md', 'README.zh-TW.md', 'README.ru.md', 'README.fa.md'];
const readmes = Object.fromEntries(names.map((name) => [name, read(name)]));
test('all five README translations match the reviewed safe guide', () => {
  assert.deepEqual(macOpenGuideViolations(guide, readmes), []);
});
test('each missing translation or mismatched README fails closed', () => {
  const sections = [...guide.matchAll(/═+\n  ([^\n]+)\n═+\n\n([\s\S]*?)(?=\n\n═+|$)/gu)];
  assert.equal(sections.length, 5);
  for (const section of sections) assert.ok(macOpenGuideViolations(guide.replace(section[0], ''), readmes).length > 0);
  for (const name of names) assert.ok(macOpenGuideViolations(guide, { ...readmes, [name]: '' }).length > 0);
});
test('removing stop guidance fails even if README is changed to match', () => {
  const changed = guide.replace('Do not clear quarantine attributes or', 'You may clear quarantine attributes or');
  const changedReadmes = { ...readmes, 'README.en.md': readmes['README.en.md'].replace('Do not clear quarantine attributes or', 'You may clear quarantine attributes or') };
  assert.ok(macOpenGuideViolations(changed, changedReadmes).some((e) => e.includes('lost safety instruction')));
});
test('bypass command cannot return in the guide or any README', () => {
  for (const command of ['xattr -cr /Applications/Polaris.app', 'spctl --master-disable']) {
    assert.ok(macOpenGuideViolations(`${guide}\n${command}`, readmes).some((e) => e.includes('security-bypass')));
    for (const name of names) assert.ok(macOpenGuideViolations(guide, { ...readmes, [name]: `${readmes[name]}\n${command}` }).some((e) => e.includes('security-bypass')));
  }
});
test('final DMG compares the entire canonical guide and keeps signature gates', () => {
  const workflow = read('.github/workflows/package.yml');
  assert.ok(workflow.includes('if ! cmp -s packaging/macos-dmg-open-guide.txt "$mnt/$guide_name"; then'));
  assert.equal(workflow.split('codesign --verify --deep --strict --verbose=2 "${apps[0]}"').length - 1, 2);
  assert.ok(workflow.includes('if ! bash scripts/macos-nested-code.sh verify "${apps[0]}" "$core_ref"; then'));
});
