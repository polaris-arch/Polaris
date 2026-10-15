import assert from 'node:assert/strict';
import { test } from 'node:test';
import { collectJsInstances, render } from './gen-third-party-licenses.mjs';
test('both versions and the second instance unique child are collected', () => {
  const values = collectJsInstances({ dependencies: {
    api: { version: '2.11.1', path: '/api-old' },
    plugin: { version: '1', dependencies: { api: { version: '2.12.0', path: '/api-new',
      dependencies: { unique: { version: '7' } } } } },
  } });
  assert.deepEqual(values.filter(p => p.name === 'api').map(p => p.value.version).sort(), ['2.11.1', '2.12.0']);
  assert.ok(values.some(p => p.name === 'unique'));
});
test('repeated instance still traverses expanded occurrences and deduplicates identity', () => {
  const values = collectJsInstances([{ dependencies: { api: { version: '1', path: '/api' } } },
    { dependencies: { api: { version: '1', path: '/api', dependencies: { child: { version: '2' } } } } }]);
  assert.equal(values.filter(p => p.name === 'api').length, 1);
  assert.ok(values.some(p => p.name === 'child'));
});
test('same version peer instances retain their distinct child closures', () => {
  const values = collectJsInstances([{ dependencies: { api: { version: '1', path: '/peer-a' } } },
    { dependencies: { api: { version: '1', path: '/peer-b', dependencies: { child: { version: '2' } } } } }]);
  assert.equal(values.filter(p => p.name === 'api').length, 2);
  assert.ok(values.some(p => p.name === 'child'));
});
test('identical license bodies remain one group containing both versions', () => {
  const text = render(['2.11.1', '2.12.0'].map(version => ({ ecosystem: 'npm', name: 'api', version,
    license: 'MIT', repository: '', texts: [{ name: 'LICENSE', text: 'fixture license body' }] })));
  assert.equal(text.split('fixture license body').length - 1, 1);
  assert.ok(text.includes('api 2.11.1') && text.includes('api 2.12.0'));
});
