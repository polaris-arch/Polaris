#!/usr/bin/env python3
"""Collect local compiler inputs using the production protected-execution producer.

This is not a workflow receipt or a signature. Only the explicit Cargo lane
consumes these fixed build inputs; formal consumers never accept this schema.
"""
import argparse, fcntl, re, hashlib, importlib, json, os, platform, shutil, stat, subprocess, sys, tarfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
SLOT=ROOT/'resources/.local-development/linux'
CONFIG=ROOT/'scripts/desktop-core/local-development-source.json'
COMPILED=ROOT/'crates/core-supervisor/src/selected_core/development_config.rs'
LIMIT=96*1024**2
MARKER=b'POLARIS_SELECTION_LOCAL_DEVELOPMENT_V1'
def demand(ok,message):
 if not ok:raise ValueError(message)
def closed(value,keys):return type(value) is dict and set(value)==set(keys)
def digest(data):return hashlib.sha256(data).hexdigest()
def encoded(value):return (json.dumps(value,sort_keys=True,indent=2)+'\n').encode()
def raw(path,limit=2*1024**2):
 s=path.lstat();demand(stat.S_ISREG(s.st_mode) and s.st_nlink==1 and s.st_uid==os.getuid() and 0<s.st_size<=limit,'unsafe/oversized owned local input: '+str(path))
 with path.open('rb') as f:
  opened=os.fstat(f.fileno());demand((opened.st_dev,opened.st_ino)==(s.st_dev,s.st_ino),'local input replaced')
  value=f.read(limit+1);after=os.fstat(f.fileno())
 demand(len(value)==s.st_size and (after.st_size,after.st_mtime_ns,after.st_ctime_ns)==(s.st_size,s.st_mtime_ns,s.st_ctime_ns),'local input changed')
 return value
def pin(path,limit=LIMIT):
 data=raw(path,limit);return {'bytes':len(data),'sha256':digest(data)}
def git(path,*args):return subprocess.check_output(['git','-C',str(path),*args],text=True,env={**os.environ,'GIT_NO_LAZY_FETCH':'1'}).strip()
def clean_source(path,expected):
 demand(path.is_dir() and not path.is_symlink() and path.stat().st_uid==os.getuid(),'source is not an owned directory')
 demand(closed(expected,['commit','tree']) and all(type(v) is str and len(v)==40 and set(v)<=set('0123456789abcdef') for v in expected.values()),'invalid compiled source pin')
 demand({'commit':git(path,'rev-parse','HEAD'),'tree':git(path,'rev-parse','HEAD^{tree}')}==expected,'source commit/tree differs')
 demand(not git(path,'status','--porcelain','--untracked-files=no'),'source is dirty')
 demand(all(line[0]=='H' for line in git(path,'ls-files','-v').splitlines()),'source has hidden/skip-worktree tracked entries')
def configuration():
 data=raw(CONFIG);c=json.loads(data)
 demand(closed(c,['schema','coreSource','producerSource','target','goVersion','tags','version','sidecar']),'local compiler configuration shape differs')
 demand(c['schema']=='polaris-local-development-source-v1' and c['target']=='linux/amd64' and platform.system()=='Linux' and platform.machine()=='x86_64','local platform is Unsupported')
 demand(type(c['tags']) is list and len(c['tags'])==len(set(c['tags'])) and all(type(t) is str and re.fullmatch('[a-z0-9_]+',t) for t in c['tags']),'invalid local tags')
 demand(type(c['version']) is str and re.fullmatch('[A-Za-z0-9.+-]{1,128}',c['version']) and '-polaris-dev.' in c['version'] and type(c['goVersion']) is str and c['goVersion'].startswith('go'),'explicit developer version/toolchain required')
 demand(closed(c['sidecar'],['bytes','sha256','version']) and type(c['sidecar']['bytes']) is int,'local sidecar configuration differs')
 return c,digest(data)
def sources(c,paths):
 demand(closed(paths,['schema','coreSourceRoot','producerSourceRoot']) and paths['schema']=='polaris-local-development-inputs-v1','local producer paths differ')
 core=Path(paths['coreSourceRoot']);producer=Path(paths['producerSourceRoot'])
 clean_source(core,c['coreSource']);clean_source(producer,c['producerSource'])
 sys.path.insert(0,str(producer/'scripts/ci'))
 n=importlib.import_module('native_release_index');t=importlib.import_module('tls_fingerprint_probe')
 demand(n.ROOT==producer and hasattr(n,'derive_local_trusted_execution'),'local producer implementation differs')
 return core,n,t

def configuration_source(doc):
 base=COMPILED.read_text().split('pub const COMPILED:')[0]
 c=doc['context'];r=doc['row']
 pins={'source_commit':c['coreSource']['commit'],'source_tree':c['coreSource']['tree'],
       'producer_commit':c['producerSource']['commit'],'producer_tree':c['producerSource']['tree'],
       'target':r['target'],'go_version':r['goVersion'],'tags':r['tags'],
       'core_bytes':r['core']['bytes'],'core_sha256':r['core']['sha256'],
       'sidecar_bytes':r['sidecar']['bytes'],'sidecar_sha256':r['sidecar']['sha256'],
       'selection_sha256':digest(encoded(doc)),'contract_sha256':doc['contractSha256']}
 fields=[key+':'+json.dumps(value) for key,value in pins.items()]
 fields.append('purposes:&'+json.dumps(doc['policy']['purposes']))
 return base+'pub const COMPILED:Option<DevelopmentSelection>=Some(DevelopmentSelection{'+','.join(fields)+'});\n'

def contract_for(doc):
 row=doc['row']
 return {'schema':'polaris-local-development-core-contract-v1','version':row['version'],'sourceCommit':row['sourceCommit'],'sourceTree':row['sourceTree'],
         'platforms':{'linux':{**{k:row[k] for k in ['core','sidecar','archive','sourceCommit','sourceTree']},'providerFingerprint':doc['providerFingerprint'],'trustedExecution':doc['policy']}}}
def binding(doc):return digest(encoded({k:doc[k] for k in ['configurationSha256','context','row','proof']}))
def inventory_for(contract,contract_bytes,sizes):
 a=contract['platforms']['linux']['archive']
 return {'schema':'polaris-core-recovery-inventory-v1','platform':'linux','contractSha256':digest(contract_bytes),
         'archive':{'name':a['name'],'format':'tar.gz','bytes':a['bytes'],'sha256':a['sha256'],'root':a['root']},'memberBytes':sizes}
def same_buildinfo(actual,row):
 go,build,graph=actual;recorded=row['rawBuildInfo'];lines=recorded.splitlines() if type(recorded) is str else []
 # The filename is an observation of the copied object, not execution identity.
 return bool(lines) and lines[0].rsplit(': ',1)[-1]==row['goVersion'] and go==row['goVersion'] and graph==row['buildGraph'] and build.splitlines()[1:]==lines[1:]

def verify():
 c,config_hash=configuration();paths=json.loads(raw(SLOT/'inputs.json'));core,n,t=sources(c,paths)
 document_raw=raw(SLOT/'selection.json');document=json.loads(document_raw)
 demand(document_raw==encoded(document),'local selection is not the exact producer encoding')
 demand(closed(document,['schema','configurationSha256','context','row','proof','policy','providerFingerprint','contractSha256']) and document['schema']=='polaris-local-development-selection-v1','local selection shape differs')
 demand(COMPILED.read_text()==configuration_source(document),'generated facts differ from the immutable Rust compiler configuration')
 demand(document['configurationSha256']==config_hash,'local compiler configuration changed')
 row=document['row'];proof=document['proof']
 demand(closed(row,['version','target','sourceCommit','sourceTree','goVersion','tags','variant','buildGraph','rawBuildInfo','archive','core','sidecar','finalPackageCronetSmoke']),'local producer row shape differs')
 demand(row['version']==c['version'] and row['goVersion']==c['goVersion'] and row['target']==c['target'] and row['tags']==','.join(c['tags']),'local build declaration differs')
 demand(document['context']['coreSource']==c['coreSource'] and document['context']['producerSource']==c['producerSource'],'local source context differs')
 for key,name in [('core','sing-box'),('sidecar','libcronet.so')]:
  actual=pin(SLOT/name);demand(row[key]=={'member':name,**actual},'final local member differs')
 demand(row['sidecar']=={'member':'libcronet.so','bytes':c['sidecar']['bytes'],'sha256':c['sidecar']['sha256']},'local sidecar is not the compiled trusted input')
 demand(same_buildinfo(n.buildinfo(SLOT/'sing-box'),row),'local actual buildinfo differs')
 policy=n.derive_local_trusted_execution(row,proof,document['context'],core)
 demand(policy==document['policy'] and document['providerFingerprint']==binding(document),'local protected facts/provider differs')
 contract_bytes=raw(SLOT/'contract.json');contract=json.loads(contract_bytes)
 demand(contract_bytes==encoded(contract_for(document)) and document['contractSha256']==digest(contract_bytes),'local compiled contract differs')
 a=row['archive'];archive=SLOT.parent/'core-recovery/linux'/a['name'];demand(encoded(pin(archive))==encoded({k:a[k] for k in ['bytes','sha256']}),'local archive bytes differ')
 root,members=n.read_archive(archive);demand(root==a['root'] and {root+'/'+k:digest(v) for k,v in members.items()}==a['members'],'local archive member closure differs')
 for key in ['core','sidecar']:
  value=members[row[key]['member']];demand({'bytes':len(value),'sha256':digest(value)}=={k:row[key][k] for k in ['bytes','sha256']},'local recovery payload differs')
 sizes={root+'/'+k:len(v) for k,v in members.items()}
 demand(raw(SLOT/'inventory.json')==encoded(inventory_for(contract,contract_bytes,sizes)),'local recovery inventory differs')
 return document

def collect(args):
 demand(not any(k in os.environ for k in ['GITHUB_ACTIONS','GITHUB_RUN_ID','GITHUB_RUN_ATTEMPT','POLARIS_NATIVE_DNS_CANDIDATE','CORE_CANDIDATE']),'local producer refuses workflow/candidate mode')
 c,config_hash=configuration();demand(not SLOT.exists(),'local producer refuses to overwrite an existing selection')
 paths={'schema':'polaris-local-development-inputs-v1','coreSourceRoot':str(args.core_source.resolve()),'producerSourceRoot':str(args.producer_source.resolve())}
 core,n,t=sources(c,paths)
 demand(not args.core.exists(),'local Core build output already exists')
 args.core.parent.mkdir(parents=True,exist_ok=True)
 command=['go','build','-mod=readonly','-buildvcs=true','-trimpath','-p=2','-tags='+','.join(c['tags']),'-ldflags=-s -w -checklinkname=0 -X runtime.godebugDefault=multipathtcp=0,tlssha1=1 -X github.com/sagernet/sing-box/constant.Version='+c['version']+' -X github.com/sagernet/sing-box/constant.SourceRevision='+c['coreSource']['commit'],'-o',str(args.core),'./cmd/sing-box']
 print('LOCAL_CORE_BUILD '+json.dumps({'argv':command,'cwd':str(core),'source':c['coreSource']},sort_keys=True),flush=True)
 result=subprocess.run(command,cwd=core,check=True)
 print('LOCAL_CORE_BUILD_EXIT '+str(result.returncode),flush=True)
 core_pin=pin(args.core);sidecar_pin=pin(args.sidecar)
 demand(sidecar_pin=={k:c['sidecar'][k] for k in ['bytes','sha256']},'sidecar bytes are not the compiled known input')
 go,build,graph=n.buildinfo(args.core)
 demand(go==c['goVersion'] and graph['settings'].get('vcs.revision')==c['coreSource']['commit'] and graph['settings'].get('vcs.modified')=='false','actual developer toolchain/source differs')
 SLOT.mkdir(parents=True,mode=0o700)
 for source,name in [(args.core,'sing-box'),(args.sidecar,'libcronet.so')]:shutil.copyfile(source,SLOT/name);os.chmod(SLOT/name,0o700 if name=='sing-box' else 0o600)
 version=c['version'];root='sing-box-'+version+'-linux-amd64';name=root+'.tar.gz'
 def normalized(info):
  info.uid=info.gid=info.mtime=0;info.uname=info.gname='';info.mode=0o755 if info.name.endswith('/sing-box') else 0o644;return info
 archive_path=SLOT.parent/'core-recovery/linux'/name;archive_path.parent.mkdir(parents=True,mode=0o700)
 with tarfile.open(archive_path,'w:gz',format=tarfile.PAX_FORMAT) as archive:
  for source,member in [(SLOT/'sing-box','sing-box'),(SLOT/'libcronet.so','libcronet.so'),(core/'LICENSE','LICENSE'),(core/'release/licenses/TLS-DEPENDENCY-NOTICES.txt','TLS-DEPENDENCY-NOTICES.txt')]:
   demand(source.is_file(),'required local source notice missing: '+str(source));archive.add(source,arcname=root+'/'+member,recursive=False,filter=normalized)
 actual_root,members=n.read_archive(archive_path)
 row={'version':version,'target':c['target'],'sourceCommit':c['coreSource']['commit'],'sourceTree':c['coreSource']['tree'],'goVersion':go,'tags':','.join(c['tags']),
      'variant':{'linkage':'shared-purego','libc':'','goarm':'','gomips':'','go386':'','legacy':False},'buildGraph':graph,'rawBuildInfo':build,
      'archive':{'name':name,**pin(archive_path),'root':actual_root,'members':{root+'/'+k:digest(v) for k,v in members.items()}},
      'core':{'member':'sing-box',**core_pin},'sidecar':{'member':'libcronet.so',**sidecar_pin}}
 with t.sealed_execution(SLOT/'sing-box',SLOT/'libcronet.so',core_pin,sidecar_pin) as execution:
  caps=t.probe(SLOT/'sing-box',source=c['coreSource']['commit'],version=version,go_version=go,target=c['target'],expected_sha=core_pin['sha256'],expected_modules=t.producer_module_identities(graph),execution=execution)
  observed=execution.run(['tools','cronet','--sha256',sidecar_pin['sha256'],'--expected-version',c['sidecar']['version']],text=True,capture_output=True,timeout=30)
  (SLOT/'cronet-observation.json').write_bytes(encoded({'argv':['tools','cronet','--sha256',sidecar_pin['sha256'],'--expected-version',c['sidecar']['version']],'exitCode':observed.returncode,'stdout':observed.stdout,'stderr':observed.stderr}))
  demand(observed.returncode==0,'local sealed-FD Cronet Version/Destroy failed')
  cronet=json.loads(observed.stdout)
  seals={'coreSeals':fcntl.fcntl(execution.core_fd,fcntl.F_GET_SEALS),'sidecarSeals':fcntl.fcntl(execution.library_fd,fcntl.F_GET_SEALS)}
 row['finalPackageCronetSmoke']={'status':'passed','os':'linux','arch':'amd64','go':go,'gvisorCompiled':True,'naiveOutbound':True,'linkage':'shared-purego','version':cronet['version'],'librarySha256':sidecar_pin['sha256'],'coreSha256':core_pin['sha256']}
 proof={**{k:row[k] for k in ['sourceCommit','sourceTree','version','target','core','sidecar']},'archiveSha256':row['archive']['sha256'],**seals,'tlsProbe':caps,'cronet':cronet}
 context={'schema':'polaris-local-development-execution-context-v1','coreSource':c['coreSource'],'producerSource':c['producerSource'],'host':{'os':'linux','arch':'amd64'}}
 policy=n.derive_local_trusted_execution(row,proof,context,core)
 doc={'schema':'polaris-local-development-selection-v1','configurationSha256':config_hash,'context':context,'row':row,'proof':proof,'policy':policy}
 doc['providerFingerprint']=binding(doc)
 contract=contract_for(doc);contract_bytes=encoded(contract);doc['contractSha256']=digest(contract_bytes)
 for path,value in [('selection.json',doc),('inputs.json',paths),('contract.json',contract),('inventory.json',inventory_for(contract,contract_bytes,{root+'/'+k:len(v) for k,v in members.items()}))]:
  (SLOT/path).write_bytes(encoded(value));os.chmod(SLOT/path,0o600)
 COMPILED.write_text(configuration_source(doc))
 verify()
 print(json.dumps({'schema':doc['schema'],'coreSource':c['coreSource'],'producerSource':c['producerSource'],'providerFingerprint':doc['providerFingerprint'],'policy':policy},sort_keys=True))
if __name__=='__main__':
 parser=argparse.ArgumentParser();parser.add_argument('mode',choices=['collect','verify']);
 for key in ['core-source','producer-source','core','sidecar']:parser.add_argument('--'+key,type=Path)
 args=parser.parse_args()
 if args.mode=='collect':
  demand(all(getattr(args,k) is not None for k in ['core_source','producer_source','core','sidecar']),'explicit local producer inputs required');collect(args)
 else:print(json.dumps({'schema':verify()['schema'],'verified':True}))
