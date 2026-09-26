import type { TsLoginMode } from './ts-login-server';

/** Saving must complete before changing modes; cancellation remains a separate action. */
export function TsLoginModeSwitch(props: {
  mode: TsLoginMode; submitting: boolean; label: string; browserLabel: string; authkeyLabel: string;
  onChange: (mode: TsLoginMode) => void;
}) {
  return <div className="seg2" role="group" aria-label={props.label} style={{ display: 'flex' }}>
    {(['browser', 'authkey'] as const).map((mode) => <button key={mode} type="button" style={{ flex: 1 }}
      className={props.mode === mode ? 'on' : ''} disabled={props.submitting}
      onClick={() => { if (!props.submitting) props.onChange(mode); }}>
      {mode === 'browser' ? props.browserLabel : props.authkeyLabel}
    </button>)}
  </div>;
}
