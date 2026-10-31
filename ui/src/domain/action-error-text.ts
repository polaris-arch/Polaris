/** 用户动作错误的稳定 code → 当前语种文案。诊断永远不直接渲染。 */
import type { BackupErrorCode, HelperActionErrorCode } from '@/ipc/api-client';

type Translate = (key: string) => string;

export function helperActionErrorText(
  code: HelperActionErrorCode | null | undefined,
  t: Translate,
): string {
  switch (code) {
    case 'cancelled': return t('helper.actionError.cancelled');
    case 'authorizationIncomplete': return t('helper.actionError.authorizationIncomplete');
    case 'partialCleanup': return t('helper.actionError.partialCleanup');
    case 'proxyRunning': return t('helper.actionError.proxyRunning');
    case 'unsupported': return t('helper.actionError.unsupported');
    case 'bundleMismatch': return t('helper.actionError.bundleMismatch');
    case 'missingAsset': return t('helper.actionError.missingAsset');
    case 'invalidInstallation': return t('helper.actionError.invalidInstallation');
    case 'tokenWriteFailed': return t('helper.actionError.tokenWriteFailed');
    case 'scriptWriteFailed': return t('helper.actionError.scriptWriteFailed');
    case 'authorizationLaunchFailed': return t('helper.actionError.authorizationLaunchFailed');
    case 'upgradeDeferred': return t('helper.actionError.upgradeDeferred');
    case 'installerFailed': return t('helper.actionError.installerFailed');
    case 'notReady': return t('helper.actionError.notReady');
    case 'recoveryRequired': return t('helper.actionError.recoveryRequired');
    default: return t('helper.actionError.failed');
  }
}

export function backupErrorText(
  code: BackupErrorCode | null | undefined,
  t: Translate,
): string {
  switch (code) {
    case 'cancelled': return t('backupImport.error.cancelled');
    case 'configLoadFailed': return t('backupImport.error.configLoadFailed');
    case 'serializeFailed': return t('backupImport.error.serializeFailed');
    case 'writeFailed': return t('backupImport.error.writeFailed');
    case 'readFailed': return t('backupImport.error.readFailed');
    case 'invalidFormat': return t('backupImport.error.invalidFormat');
    case 'invalidArgs': return t('backupImport.error.invalidArgs');
    case 'sharedDependencyConflict': return t('backupImport.error.sharedDependencyConflict');
    case 'saveFailed': return t('backupImport.error.saveFailed');
    case 'meshRoutingRequiresTrustedRestore': return t('backupImport.error.meshRoutingRequiresTrustedRestore');
    case 'meshRoutingInvalidPolicy': return t('backupImport.error.meshRoutingInvalidPolicy');
    default: return t('backupImport.error.unknown');
  }
}

/** 节点切换错误码 → 用户可操作的当前语种说明；后端诊断只进入日志。 */
export function serverSwitchErrorText(
  code: string | null | undefined,
  t: Translate,
): string {
  if (code === 'SERVER_SWITCH_READBACK_FAILED') return t('autoSelect.error.SERVER_SWITCH_READBACK_FAILED');
  if (code === 'serverIdentityAmbiguous') return t('identityRepair.ambiguous');
  if (code === 'OUTBOUND_INTERFACE_UNAVAILABLE') {
    return t('home.outboundInterfaceUnavailable');
  }
  return t('home.switchError');
}

/** Configuration validation failures use stable codes, never arbitrary backend messages. */
export function configSaveErrorText(error: unknown, t: Translate): string {
  const code = typeof error === 'object' && error !== null && 'code' in error ? error.code : undefined;
  return t(code === 'DNS_DEPENDENCY_INVALID' ? 'errors.dnsDependencyInvalid' : 'common.saveFailed');
}

/** Read only structured codes; native diagnostics may contain private node details. */
export function serverSwitchErrorCode(error: unknown): string | undefined {
  return error && typeof error === 'object' && 'code' in error && typeof error.code === 'string' ? error.code : undefined;
}
