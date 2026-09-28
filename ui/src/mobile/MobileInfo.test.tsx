import { afterEach, describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { MobileInfo, MobileInfoPanel } from './MobileInfo';
import { clearDismissables, dismissTop } from './back-stack';
import { FormSheet } from './forms/FormSheet';

afterEach(clearDismissables);

describe('mobile explanatory details', () => {
  it('keeps the explicit consequence visible and the full explanation behind one correctly named i', () => {
    const html = renderToStaticMarkup(<MobileInfo title="Exit" triggerLabel="Read exit details"
      summary="Internet still goes direct" details={<p>Full explanation</p>} tone="warn" />);
    expect(html).toContain('Internet still goes direct');
    expect(html).toContain('aria-label="Read exit details"');
    expect(html).toContain('aria-haspopup="dialog"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain('M12 11v5M12 8h.01');
    expect(html).not.toContain('Full explanation');
    expect(html).not.toContain('role="dialog"');
  });

  it('does not add another summary when a card already states its status', () => {
    const html = renderToStaticMarkup(<MobileInfo title="Node" triggerLabel="Read node details" details="Explanation" />);
    expect(html).toContain('m-info-icon-only');
    expect(html).not.toContain('m-info-summary');
    expect((html.match(/<button/g) ?? []).length).toBe(1);
  });

  it('back dismisses only the details above an existing form; complete content and close remain reachable', () => {
    let formClosed = 0;
    let detailsClosed = 0;
    const html = renderToStaticMarkup(<>
      <FormSheet title="Edit node" closeLabel="Close form" cancelLabel="Cancel" onRequestClose={() => { formClosed += 1; }}>Fields</FormSheet>
      <MobileInfoPanel title="Node details" closeLabel="Close details" onClose={() => { detailsClosed += 1; }}>
        <section><h3>Speed test</h3><p>Complete explanation</p></section>
      </MobileInfoPanel>
    </>);
    expect(html).toContain('Complete explanation');
    const details = html.slice(html.indexOf('class="m-form-layer m-info-layer"'));
    expect(details).toContain('aria-modal="true"');
    expect(details).toContain('m-form-head');
    expect(details).toContain('m-info-body');
    expect(details).not.toContain('m-form-foot');
    expect(dismissTop()).toBe(true);
    expect(detailsClosed).toBe(1);
    expect(formClosed).toBe(0);
    expect(dismissTop()).toBe(true);
    expect(formClosed).toBe(1);
  });
});
