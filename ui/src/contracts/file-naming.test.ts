/** 大小写不敏感的平台不能安全检出相互折叠的仓库路径。 */
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const REPO = fileURLToPath(new URL('../../../', import.meta.url));
function repositoryFiles(): string[] {
  return execFileSync(
    'git',
    ['ls-files', '--cached', '--others', '--exclude-standard', '--deduplicate'],
    { cwd: REPO, encoding: 'utf8' },
  )
    .split('\n')
    .filter((path) => path && existsSync(join(REPO, path)));
}

describe('文件系统命名', () => {
  const files = repositoryFiles();

  it('不存在大小写折叠后的路径冲突', () => {
    const seen = new Map<string, string>();
    const collisions: string[] = [];
    for (const path of files) {
      const folded = path.normalize('NFC').toLowerCase();
      const previous = seen.get(folded);
      if (previous && previous !== path) collisions.push(`${previous} <> ${path}`);
      seen.set(folded, path);
    }
    expect(collisions, `大小写不敏感平台会把这些路径视为同一文件：\n${collisions.join('\n')}`).toEqual([]);
  });

});
