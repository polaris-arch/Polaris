import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const read = (rel: string) =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');

describe('主内容页页头', () => {
  it('导入节点首次点击直接打开粘贴文本弹窗，不在挂载时读取剪贴板', () => {
    const dialog = read('../dialogs/ImportDialog.tsx');
    expect(dialog).not.toContain('navigator.clipboard.readText()');
  });
});
