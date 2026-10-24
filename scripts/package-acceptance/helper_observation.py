"""Bounded non-secret projection of real callback values; never an IPC substitute.

Native invoke is sealed. Shape recognition cannot establish command identity or
permission success. The consumer must preserve that distinction and its original
business assertions. No message/diagnostic/arguments/path is retained.
"""
import json

SCHEMA = 'polaris-helper-callback-projection-v1'
ERROR_CODES = (
    'cancelled', 'authorizationIncomplete', 'partialCleanup', 'proxyRunning',
    'unsupported', 'missingAsset', 'bundleMismatch', 'notReady',
    'invalidInstallation', 'tokenWriteFailed', 'scriptWriteFailed',
    'authorizationLaunchFailed', 'installerFailed', 'upgradeDeferred', 'failed',
)
STATUS_BOOLS = ('supported', 'installed', 'ready', 'upgradeable', 'needsRepair',
                'backgroundDisabled', 'pathMismatch')
CORE_STATES = ('present', 'absent', 'invalid', 'unavailable')

# A pure expression for the existing callback Map.get observer. It neither
# replaces invoke nor reads callback ids, arguments, exceptions or native state.
PROJECTOR_JS = r'''((raw) => {
 const codes=__CODES__, bools=__BOOLS__, cores=__CORES__;
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
 const out={schema:'polaris-helper-callback-projection-v1',kind,
  association:kind==='outerFailure'?'uncorrelated':'shape-only',
  envelopeSuccess:b(raw.success),actionSuccess:action?b(d.success):null,
  errorCode:!action?'not-applicable':!own(d,'errorCode')?'missing':d.errorCode===null?'none':codes.includes(d.errorCode)?d.errorCode:'unknown',
  statusShape:s===null?'not-applicable':bools.slice(0,3).every(k=>typeof s[k]==='boolean')?'valid':'malformed',
  status:Object.fromEntries(bools.map(k=>[k,s===null?null:b(s[k])])),
  selectedCoreState:!s||!own(s,'selectedCore')?'missing':s.selectedCore===null?'none':obj(s.selectedCore)&&cores.includes(s.selectedCore.state)?s.selectedCore.state:'unknown',
  protocolVersion:s&&Number.isInteger(s.version)&&s.version>=0&&s.version<=4294967295?s.version:null,
  expectedProtocolVersion:s&&Number.isInteger(s.expectedProtocolVersion)&&s.expectedProtocolVersion>=0&&s.expectedProtocolVersion<=4294967295?s.expectedProtocolVersion:null,
  buildIdentityMatches:s&&typeof s.helperBuildId==='string'&&typeof s.expectedBuildId==='string'?s.helperBuildId===s.expectedBuildId:null};
 return out;
})'''.replace('__CODES__', json.dumps(ERROR_CODES)).replace('__BOOLS__', json.dumps(STATUS_BOOLS)).replace('__CORES__', json.dumps(CORE_STATES))


def validate_projection(value):
    """Reject polluted/malformed observations, return a detached fixed record."""
    keys = {'schema', 'kind', 'association', 'envelopeSuccess', 'actionSuccess',
            'errorCode', 'statusShape', 'status', 'selectedCoreState',
            'protocolVersion', 'expectedProtocolVersion', 'buildIdentityMatches'}
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
    return {**value, 'status': dict(value['status'])}
