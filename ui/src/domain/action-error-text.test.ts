import { describe, expect, it } from 'vitest';
import {
  backupErrorText,
  configSaveErrorText,
  helperActionErrorText,
  serverSwitchErrorText,
} from './action-error-text';

const t = (key: string) => `translated:${key}`;

describe('structured action error text', () => {
  it('stable helper code wins over any diagnostic', () => {
    expect(helperActionErrorText('bundleMismatch', t)).toBe('translated:helper.actionError.bundleMismatch');
    expect(helperActionErrorText('missingAsset', t)).toBe('translated:helper.actionError.missingAsset');
    expect(helperActionErrorText('authorizationIncomplete', t)).toBe(
      'translated:helper.actionError.authorizationIncomplete'
    );
    expect(helperActionErrorText('proxyRunning', t)).toBe(
      'translated:helper.actionError.proxyRunning'
    );
  });

  it('partial cleanup stays separate from success or cancellation', () => {
    expect(helperActionErrorText('partialCleanup', t)).toBe('translated:helper.actionError.partialCleanup');
  });

  it('unknown or future backup codes use a safe localized fallback', () => {
    expect(backupErrorText(undefined, t)).toBe('translated:backupImport.error.unknown');
    expect(backupErrorText('futureCode' as never, t)).toBe('translated:backupImport.error.unknown');
  });

  it('maps an unavailable outbound interface without exposing backend diagnostics', () => {
    expect(serverSwitchErrorText('OUTBOUND_INTERFACE_UNAVAILABLE', t)).toBe(
      'translated:home.outboundInterfaceUnavailable'
    );
    expect(serverSwitchErrorText('futureCode', t)).toBe('translated:home.switchError');
  });
});

it('maps shared dependency conflicts to actionable localized backup copy', () => {
  expect(backupErrorText('sharedDependencyConflict', t)).toBe('translated:backupImport.error.sharedDependencyConflict');
});

it.each(["invalidInstallation", "tokenWriteFailed", "scriptWriteFailed", "authorizationLaunchFailed", "installerFailed", "upgradeDeferred"])('preserves Helper installation stage %s', code => {
  expect(helperActionErrorText(code as Parameters<typeof helperActionErrorText>[0], t)).toBe(`translated:helper.actionError.${code}`);
});


describe('safe DNS dependency error feedback', () => {
  it('uses only the stable code and never displays credential-bearing diagnostics', () => {
    expect(configSaveErrorText({ code: 'DNS_DEPENDENCY_INVALID', message: 'https://user:secret@example.invalid' }, t))
      .toBe('translated:errors.dnsDependencyInvalid');
    expect(configSaveErrorText({ message: 'DNS_DEPENDENCY_INVALID https://user:secret@example.invalid' }, t))
      .toBe('translated:common.saveFailed');
  });
});

it('explains duplicate identity recovery without exposing backend diagnostics', () => {
  expect(serverSwitchErrorText('serverIdentityAmbiguous', key => key)).toBe('identityRepair.ambiguous');
});
