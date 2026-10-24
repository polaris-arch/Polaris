"""Bounded non-secret projection of real callback values; never an IPC substitute.

Native invoke is sealed. Shape recognition cannot establish command identity or
permission success. The consumer must preserve that distinction and its original
business assertions. No message/diagnostic/arguments/path is retained.
"""
import json

SCHEMA = 'polaris-helper-callback-projection-v2'
ERROR_CODES = (
    'cancelled', 'authorizationIncomplete', 'partialCleanup', 'proxyRunning',
    'unsupported', 'missingAsset', 'bundleMismatch', 'notReady',
    'invalidInstallation', 'tokenWriteFailed', 'scriptWriteFailed',
    'authorizationLaunchFailed', 'installerFailed', 'upgradeDeferred', 'failed',
)
STATUS_BOOLS = ('supported', 'installed', 'ready', 'upgradeable', 'needsRepair',
                'backgroundDisabled', 'pathMismatch')
CORE_STATES = ('present', 'absent', 'invalid', 'unavailable')
EXECUTION_MARKERS = ('unknown', 'notSetuidRoot', 'noAuthenticationAgent', 'notAuthorized',
                     'textAgentCreationFailed', 'textAgentRegistrationFailed', 'authorizationCheckFailed')
EXECUTION_SCHEMA = 'polaris-helper-execution-diagnostic-v1'
EXECUTION_SCOPE = 'child-stderr-marker-not-verified-emitter'

INSTALLER_STAGES = ('preparation', 'directories', 'staging', 'lease', 'journal', 'core-install', 'policy',
    'service-snapshot', 'manager-identity', 'manager-property', 'pid1-cgroup', 'cgroup-mount', 'unit-config',
    'service-stop', 'service-closure', 'registration', 'service-install', 'service-start', 'takeover',
    'uninstall-remove', 'cleanup', 'rollback', 'unknown')
INSTALLER_RECOVERY = ('not-started', 'cleanup-returned', 'rollback-returned', 'recovery-unconfirmed')

# A pure expression for the existing callback Map.get observer. It neither
# replaces invoke nor reads callback ids, arguments, exceptions or native state.
PROJECTOR_JS = r'''((raw) => {
 const codes=__CODES__, bools=__BOOLS__, cores=__CORES__, markers=__MARKERS__;
 const stages=__STAGES__, recoveries=__RECOVERIES__;
 const obj=v=>v!==null&&typeof v==='object'&&!Array.isArray(v);
 const own=(v,k)=>obj(v)&&Object.hasOwn(v,k);
 const b=v=>typeof v==='boolean'?v:null;
 if(!obj(raw)) return null;
 const d=raw.data;
 const action=obj(d)&&obj(d.status)&&(own(d.status,'supported')||own(d.status,'installed'));
 const status=obj(d)&&(own(d,'supported')&&own(d,'installed'));
 if(!action&&!status&&raw.success!==false) return null;
 const kind=action?'helperAction':status?'helperStatus':'outerFailure';
 const s=action?d.status:status?d:null;
 const e=action?d.executionDiagnostic:null;
 const ePresent=action&&own(d,'executionDiagnostic')&&e!==null;
 const f=own(e,'installerFailure')?e.installerFailure:undefined;
 const fValid=f===undefined||(e.exitCode!==0&&obj(f)&&Object.keys(f).length===3&&f.schema==='polaris-helper-installer-failure-v1'&&stages.includes(f.stage)&&recoveries.includes(f.recovery));
 const eValid=ePresent&&d.success===false&&obj(e)&&[4,5].includes(Object.keys(e).length)&&Object.keys(e).every(k=>['schema','exitCode','marker','scope','installerFailure'].includes(k))&&fValid&&
  e.schema==='polaris-helper-execution-diagnostic-v1'&&e.scope==='child-stderr-marker-not-verified-emitter'&&
  Number.isInteger(e.exitCode)&&e.exitCode>=-1&&e.exitCode<=255&&markers.includes(e.marker)&&
  (e.exitCode===127||e.marker==='unknown');
 const out={schema:'polaris-helper-callback-projection-v2',kind,
  association:kind==='outerFailure'?'uncorrelated':'shape-only',
  envelopeSuccess:b(raw.success),actionSuccess:action?b(d.success):null,
  executionDiagnosticState:!action?'not-applicable':!ePresent?'absent':eValid?'valid':'malformed',
  executionDiagnostic:eValid?{schema:e.schema,exitCode:e.exitCode,marker:e.marker,scope:e.scope,...(f?{installerFailure:{schema:f.schema,stage:f.stage,recovery:f.recovery}}:{})}:null,
  errorCode:!action?'not-applicable':!own(d,'errorCode')?'missing':d.errorCode===null?'none':codes.includes(d.errorCode)?d.errorCode:'unknown',
  statusShape:s===null?'not-applicable':bools.slice(0,3).every(k=>typeof s[k]==='boolean')?'valid':'malformed',
  status:Object.fromEntries(bools.map(k=>[k,s===null?null:b(s[k])])),
  selectedCoreState:!s||!own(s,'selectedCore')?'missing':s.selectedCore===null?'none':obj(s.selectedCore)&&cores.includes(s.selectedCore.state)?s.selectedCore.state:'unknown',
  protocolVersion:s&&Number.isInteger(s.version)&&s.version>=0&&s.version<=4294967295?s.version:null,
  expectedProtocolVersion:s&&Number.isInteger(s.expectedProtocolVersion)&&s.expectedProtocolVersion>=0&&s.expectedProtocolVersion<=4294967295?s.expectedProtocolVersion:null,
  buildIdentityMatches:s&&typeof s.helperBuildId==='string'&&typeof s.expectedBuildId==='string'?s.helperBuildId===s.expectedBuildId:null};
 return out;
})'''.replace('__CODES__', json.dumps(ERROR_CODES)).replace('__BOOLS__', json.dumps(STATUS_BOOLS)).replace('__CORES__', json.dumps(CORE_STATES)).replace('__MARKERS__', json.dumps(EXECUTION_MARKERS)).replace('__STAGES__', json.dumps(INSTALLER_STAGES)).replace('__RECOVERIES__', json.dumps(INSTALLER_RECOVERY))


def validate_projection(value):
    """Reject polluted/malformed observations, return a detached fixed record."""
    keys = {'schema', 'kind', 'association', 'envelopeSuccess', 'actionSuccess',
            'errorCode', 'statusShape', 'status', 'selectedCoreState',
            'protocolVersion', 'expectedProtocolVersion', 'buildIdentityMatches',
            'executionDiagnosticState', 'executionDiagnostic'}
    def need(condition):
        if not condition:
            raise ValueError('invalid helper callback projection')
    need(type(value) is dict and set(value) == keys)
    need(value['schema'] == SCHEMA)
    need(value['kind'] in ('helperAction', 'helperStatus', 'outerFailure'))
    action = value['kind'] == 'helperAction'
    outer = value['kind'] == 'outerFailure'
    need(value['association'] == ('uncorrelated' if outer else 'shape-only'))
    for key in ('envelopeSuccess', 'actionSuccess', 'buildIdentityMatches'):
        need(value[key] is None or type(value[key]) is bool)
    need(action or value['actionSuccess'] is None)
    state = value['executionDiagnosticState']; execution = value['executionDiagnostic']
    need(state in (('absent', 'valid', 'malformed') if action else ('not-applicable',)))
    if state == 'valid':
        need(value['actionSuccess'] is False and type(execution) is dict and
             set(execution) in ({'schema', 'exitCode', 'marker', 'scope'}, {'schema', 'exitCode', 'marker', 'scope', 'installerFailure'}))
        if 'installerFailure' in execution:
            f = execution['installerFailure']
            need(type(f) is dict and set(f) == {'schema', 'stage', 'recovery'} and
                 f['schema'] == 'polaris-helper-installer-failure-v1' and
                 f['stage'] in INSTALLER_STAGES and f['recovery'] in INSTALLER_RECOVERY and execution['exitCode'] != 0)
        need(execution['schema'] == EXECUTION_SCHEMA and execution['scope'] == EXECUTION_SCOPE and
             type(execution['exitCode']) is int and -1 <= execution['exitCode'] <= 255 and
             execution['marker'] in EXECUTION_MARKERS and
             (execution['exitCode'] == 127 or execution['marker'] == 'unknown'))
    else:
        need(execution is None)
    need(value['errorCode'] in (ERROR_CODES + ('none', 'missing', 'unknown') if action else ('not-applicable',)))
    need(value['statusShape'] in (('not-applicable',) if outer else ('valid', 'malformed')))
    need(type(value['status']) is dict and set(value['status']) == set(STATUS_BOOLS))
    need(all(v is None or type(v) is bool for v in value['status'].values()))
    if value['statusShape'] == 'valid':
        need(all(type(value['status'][k]) is bool for k in STATUS_BOOLS[:3]))
    need(value['selectedCoreState'] in CORE_STATES + ('none', 'missing', 'unknown'))
    for key in ('protocolVersion', 'expectedProtocolVersion'):
        need(value[key] is None or (type(value[key]) is int and 0 <= value[key] <= 4294967295))
    if outer:
        need(value['envelopeSuccess'] is False)
        need(all(v is None for v in value['status'].values()))
        need(value['selectedCoreState'] == 'missing')
        need(all(value[k] is None for k in ('protocolVersion', 'expectedProtocolVersion', 'buildIdentityMatches')))
    return {**value, 'status': dict(value['status']),
            'executionDiagnostic': json.loads(json.dumps(execution)) if execution is not None else None}
