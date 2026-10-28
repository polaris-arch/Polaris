import type { AutoSelectSwitchView } from '@/mobile/AutoSelectControls';
import { SetRow, SetRowGroup, Switch } from './Primitives';

/** Keep the setting compact; transient status/actions belong to its subordinate area. */
export function AutoSelectSettingsSwitch({ label, tip, feedback, checked, disabled, onChange }: AutoSelectSwitchView) {
  return <SetRowGroup>
    <SetRow label={label} tip={tip}>
      <Switch checked={checked} disabled={disabled} onChange={onChange} aria-label={label} />
    </SetRow>
    {feedback != null && <div className="set-row-details">{feedback}</div>}
  </SetRowGroup>;
}
