import { afterAll, describe, expect, it } from 'vitest';
import { closeOracle, cssBundleScan, measure } from '@/styles/css-oracle.test-support';
import { inMobileShell } from '@/styles/mount.test-support';

afterAll(closeOracle);
describe('mobile document scrollbar policy', () => {
  it('the document-wide policy belongs to the mobile bundle, while desktop keeps its existing CSS chain', () => {
    expect(cssBundleScan('src/mobile/MobileMain.tsx').units).toContain('src/mobile/mobile.css');
    expect(cssBundleScan('src/main.tsx').units).not.toContain('src/mobile/mobile.css');
  });
  it('the actual mobile CSS hides document, content and textarea chrome without changing overflow', async () => {
    const html=inMobileShell('<div class="scroll-fixture" style="height:40px;overflow:auto"><p style="height:400px">Content</p></div>'
      +'<div class="list-fixture" style="height:40px;overflow:auto"><textarea style="overflow:auto">Text</textarea><p style="height:400px">List</p></div>', 'home');
    const selectors=['html','body','.scroll-fixture','.list-fixture','textarea'];
    const measured=await measure({ctx:'mobile',html},selectors.flatMap(select=>[
      {select,props:['scrollbar-width','overflow-y']},{select,pseudo:'::-webkit-scrollbar',props:['display']},
    ]));
    for(const select of selectors){
      expect(measured.get(select,'scrollbar-width')).toBe('none');
      expect(measured.get(select,'display','::-webkit-scrollbar')).toBe('none');
    }
    for(const select of selectors.slice(2)) expect(measured.get(select,'overflow-y')).toBe('auto');
  });
});
