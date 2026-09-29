/**
 * 导入预览的**桌面 ↔ 移动端对差**门。
 *
 * # 守的是什么
 *
 * 导入拆成「解析 → 预览 → 导入 N 项」两步的全部理由，是后端返回的 `stats` 与 `warnings` 不能被丢掉
 * （见 `ImportFormPanel.tsx` 头注）。本地 sing-box 文件导入 MASQUE / Tailcat 之后，这两样承载的事实
 * 更多了：生成侧会拒收的节点计进 `stats.failed`、生成侧会剥掉的键进 `warnings`、证书固定只比服务器
 * 证书本身的提示也走 `warnings`（`cert_pin_import_warning`）。移动端预览此前只画了 `warnings`，
 * 「已跳过 N 项」「N 个协议当前内核不支持」「不支持」徽标一样都没有 —— 用户在手机上导入一份
 * 含 3 个坏 MASQUE 节点的文件，预览里看不出少了 3 个。
 *
 * # 取材面（从桌面源码现场抽，不手抄名单）
 *
 * 桌面 `ImportDialog.tsx` 的预览块（`{preview && (` → `</Modal>`）：
 *  I1 其中每一个 `t('…')` 键，移动端 `ImportFormPanel.tsx`（剥注释后）都必须渲染；
 *  I2 其中读到的每一个 `preview.<字段路径>`（`.length` 折掉），移动端都必须读。
 * 自检：两份取材各自非空，且含已知项（`import.nodesTitle` / `warnings`），防截段失配后空跑恒绿。
 *
 * # 文件腿（2026-09-25 ζ 批 A7 补）
 *
 * 移动端此前**只有粘贴**：头注写着「文件选择腿不在本批」，而那条理由早已过期（备份页的 SAF 选择器
 * 已接、后端 `local_import_pick_file` 已兼容 content URI）。屏级登记表只覆盖屏，不覆盖桌面
 * `ImportDialog` ⇒ 这一格不在任何门的射程里。补在这里而不是 `node-form-parity` 那套全量指纹：
 * 全量指纹会把 `ImportDialog` 的拖拽 / 来源切换 / 订阅链接分流等一整批触屏上不存在或另有处置的
 * 能力一并拖进来逐条登记，而本门要回答的只有一句「桌面的文件选择那条腿，移动端有没有」。
 * 取材同样现场抽：桌面 `const pickFile = async` → `const requestClose` 那一段，
 *  F1 其中每一个 `api.<链>(` 调用，移动端 `pickFile` 段都要调；
 *  F2 其中每一个 `t('…')` 键（过大 / 读失败），移动端 `pickFile` 段都要渲染；
 *  F3 移动端 `pickFile` 段把读回的正文喂进**同一条** `parse(` 预览链，且真有一颗控件调它。
 *
 * # 射程外（如实登记）
 *
 * 源码级对差：证明移动端引用了同一批键与字段，不证明它们在真机上画对了位置（真机验收）。
 * `data-tip` 悬浮提示在触屏上没有等价物，移动端把同一条键渲染成可见说明 —— 本门只认「键被渲染」。
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { blankComments } from '@/contracts/comment-blanking.test-support';

const read = (rel: string): string => blankComments(readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8'));

const DESKTOP = read('../../components/dialogs/ImportDialog.tsx');
const MOBILE = read('./ImportFormPanel.tsx');

function desktopPreviewBlock(): string {
  const a = DESKTOP.indexOf('{preview && (');
  const b = a < 0 ? -1 : DESKTOP.indexOf('</Modal>', a);
  if (a < 0 || b < 0) throw new Error('ImportDialog.tsx 里截不到预览块（`{preview && (` → `</Modal>`）—— 结构改了，本门失去取材面');
  return DESKTOP.slice(a, b);
}

const BLOCK = desktopPreviewBlock();
const KEYS = [...new Set([...BLOCK.matchAll(/\bt\(\s*'([\w.]+)'/g)].map((m) => m[1]))].sort();
const FIELDS = [
  ...new Set([...BLOCK.matchAll(/\bpreview\.([\w.]+)/g)].map((m) => m[1].replace(/\.length$/, ''))),
].sort();

describe('导入预览：移动端渲染桌面预览块的全部键与字段', () => {
  it('取材自检：两份取材非空且含已知项', () => {
    expect(KEYS, '桌面预览块一个 t() 键都没抽到').toContain('import.nodesTitle');
    expect(FIELDS, '桌面预览块没读 warnings —— 取材失配').toContain('warnings');
    expect(FIELDS).toContain('stats.failed');
  });

  it('I1 桌面预览块的每个文案键，移动端都渲染', () => {
    const missing = KEYS.filter((k) => !new RegExp(`\\bt\\(\\s*'${k.replace(/\./g, '\\.')}'`).test(MOBILE));
    expect(missing, '移动端导入预览缺这些文案（桌面有）').toEqual([]);
  });

  it('I2 桌面预览块读的每个结果字段，移动端都读', () => {
    const missing = FIELDS.filter((f) => !MOBILE.includes(`preview.${f}`));
    expect(missing, '移动端导入预览没读这些后端结果字段 —— 那一部分事实被丢掉了').toEqual([]);
  });
});

/** 截一段；截不到返回空串（移动端缺这条腿时要让下面的断言带着理由红，而不是整个文件加载失败）。 */
function sliceOrEmpty(src: string, start: string, end: string): string {
  const a = src.indexOf(start);
  const b = a < 0 ? -1 : src.indexOf(end, a);
  return a < 0 || b < 0 ? '' : src.slice(a, b);
}

const DESKTOP_PICK = sliceOrEmpty(DESKTOP, 'const pickFile = async', 'const requestClose');
const MOBILE_PICK = sliceOrEmpty(MOBILE, 'const pickFile = async', 'const doImport');
const PICK_APIS = [...new Set([...DESKTOP_PICK.matchAll(/\bapi\.([\w.]+)\s*\(/g)].map((m) => m[1]))].sort();
const PICK_KEYS = [...new Set([...DESKTOP_PICK.matchAll(/\bt\(\s*'([\w.]+)'/g)].map((m) => m[1]))].sort();

describe('导入文件腿：桌面 ImportDialog 的文件选择，移动端 ImportFormPanel 也有', () => {
  it('取材自检：桌面文件腿截得到，且含已知的 IPC 与两条错误文案', () => {
    expect(DESKTOP_PICK, 'ImportDialog.tsx 里截不到 `const pickFile = async` → `const requestClose`').not.toBe('');
    expect(PICK_APIS).toContain('localImport.pickFile');
    expect(PICK_KEYS).toEqual(expect.arrayContaining(['import.fileTooLarge', 'import.fileReadFail']));
  });

  it('F1 桌面文件腿调的每个 IPC，移动端文件腿都调', () => {
    expect(MOBILE_PICK, '移动端 ImportFormPanel 没有 `pickFile` 这条腿 —— 只能粘贴').not.toBe('');
    const missing = PICK_APIS.filter((a) => !MOBILE_PICK.includes(`api.${a}(`));
    expect(missing, '移动端文件腿缺这些 IPC 调用（桌面有）').toEqual([]);
  });

  it('F2 桌面文件腿的每个文案键，移动端文件腿都渲染', () => {
    const missing = PICK_KEYS.filter((k) => !new RegExp(`\\bt\\(\\s*'${k.replace(/\./g, '\\.')}'`).test(MOBILE_PICK));
    expect(missing, '移动端文件腿缺这些失败文案 —— 过大 / 读失败会变成静默').toEqual([]);
  });

  it('F3 读回的正文走同一条 parse 预览链，且真有一颗控件调它', () => {
    expect(MOBILE_PICK, '文件腿读回正文之后没进 `parse(` —— 另起了一条预览，或干脆没预览').toMatch(/\bparse\(\s*r\.content\s*\)/);
    expect(MOBILE, '没有任何控件调用 `pickFile` —— 腿写了但点不到').toMatch(/onClick=\{\(\)\s*=>\s*void pickFile\(\)\}/);
  });
});
