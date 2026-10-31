"""Pure source controls; no fixture is an execution policy/native acceptance."""
import importlib.util, json, tempfile, unittest
from pathlib import Path
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('local',Path(__file__).with_name('local-development.py'))
n=importlib.util.module_from_spec(spec);spec.loader.exec_module(n)
class LocalCompilerInputs(unittest.TestCase):
 def test_context_and_source_configuration_do_not_accept_workflow_or_unknown_lanes(self):
  with tempfile.TemporaryDirectory() as d:
   p=Path(d)/'config.json'
   original=json.loads(n.CONFIG.read_text())
   for key,value in [('runId',42),('validationOnly',False),('lane','Formal'),('purpose','RunMain')]:
    p.write_text(json.dumps({**original,key:value}))
    with self.subTest(key=key),patch.object(n,'CONFIG',p),self.assertRaisesRegex(ValueError,'configuration shape'):n.configuration()
 def test_the_generated_compiler_configuration_binds_policy_bytes_and_every_source_revision(self):
  proof={'configurationSha256':'0'*64,'context':{'coreSource':{'commit':'a'*40,'tree':'b'*40},'producerSource':{'commit':'c'*40,'tree':'d'*40}},
         'row':{'target':'linux/amd64','goVersion':'go1.27.2','tags':'with_purego','core':{'bytes':7,'sha256':'1'*64},'sidecar':{'bytes':11,'sha256':'2'*64}},
         'policy':{'purposes':['CheckConfig']},'contractSha256':'3'*64}
  before=n.configuration_source(proof)
  proof['policy']['purposes'].append('RunMain')
  after=n.configuration_source(proof)
  self.assertNotEqual(before,after)
  self.assertIn('producer_commit:"'+('c'*40)+'"',after)
  self.assertIn('source_commit:"'+('a'*40)+'"',after)
 def test_copy_filename_is_not_build_identity_but_all_actual_settings_still_are(self):
  graph={'settings':{'vcs.revision':'a'*40,'CGO_ENABLED':'0'}}
  recorded={'goVersion':'go1.27.2','rawBuildInfo':'original: go1.27.2\n\tbuild\tCGO_ENABLED=0\n','buildGraph':graph}
  copied=('go1.27.2','copied: go1.27.2\n\tbuild\tCGO_ENABLED=0\n',graph)
  self.assertTrue(n.same_buildinfo(copied,recorded))
  self.assertFalse(n.same_buildinfo(('go1.28.0',copied[1],graph),recorded))
  self.assertFalse(n.same_buildinfo((copied[0],copied[1].replace('CGO_ENABLED=0','CGO_ENABLED=1'),graph),recorded))
  self.assertFalse(n.same_buildinfo((copied[0],copied[1],{'settings':{'vcs.revision':'b'*40,'CGO_ENABLED':'0'}}),recorded))
  self.assertFalse(n.same_buildinfo(copied,{**recorded,'rawBuildInfo':recorded['rawBuildInfo'].replace('go1.27.2','go1.26.0')}))
 def test_local_source_gate_rejects_hidden_git_entries_and_dirty_source(self):
  source=Path(__file__).resolve().parents[2]
  identity={'commit':'a'*40,'tree':'b'*40}
  for status,files in [('', 'h scripts/hidden.py'),('', 'S skipped'),(' M go.mod','H go.mod')]:
   def git(path,*args):
    if args==('rev-parse','HEAD'):return identity['commit']
    if args==('rev-parse','HEAD^{tree}'):return identity['tree']
    if args[0]=='status':return status
    return files
   with self.subTest(status=status,files=files),patch.object(n,'git',side_effect=git),self.assertRaises(ValueError):n.clean_source(source,identity)


# Actual owned compressed fixtures. No App/Helper code is executed.
import os, shutil, subprocess, zipfile, sys
artifact_spec=importlib.util.spec_from_file_location('package',Path(__file__).resolve().parents[1]/'final-package-receipt.py')
package=importlib.util.module_from_spec(artifact_spec);artifact_spec.loader.exec_module(package)
class FormalArtifactSelection(unittest.TestCase):
 marker=b'POLARIS_SELECTION_LOCAL_DEVELOPMENT_V1'
 def stage(self,root,local=False):
  for name,data in [('usr/bin/polaris',b'ordinary App'+(self.marker if local else b'')),('usr/lib/Polaris/_up_/resources/linux/polaris-helper',b'ordinary Helper')]:
   p=root/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(data)
 def test_actual_compressed_zip_and_apk_members_not_source_or_digest_labels(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=Path(tmp)
   for local in [False,True]:
    archive=root/('local.zip' if local else 'formal.zip')
    with zipfile.ZipFile(archive,'w',compression=zipfile.ZIP_DEFLATED) as z:
     z.writestr('polaris.exe',b'a'*65530+(self.marker if local else b'ordinary App'))
     z.writestr('_up_/resources/win/polaris-helper.exe',b'ordinary Helper')
    if local:
     with self.assertRaisesRegex(ValueError,'LocalDevelopmentSelection executable'):package.reject_local_selection(archive)
     child=subprocess.run([sys.executable,'-B',str(Path(package.__file__)),'--reject-local-selection',str(archive)],capture_output=True)
     self.assertNotEqual(child.returncode,0);self.assertIn(b'LocalDevelopmentSelection executable',child.stderr)
    else:self.assertEqual(len(package.reject_local_selection(archive)['checkedExecutables']),2)
   apk=root/'a.apk'
   with zipfile.ZipFile(apk,'w') as z:z.writestr('lib/arm64-v8a/libpolaris_lib.so',self.marker)
   with self.assertRaisesRegex(ValueError,'LocalDevelopmentSelection'):package.reject_local_selection(apk)
 def test_nonproc_snapshot_checks_growth_and_deadline_before_each_write(self):
  # Real retained source and scratch writes; force only the non-proc branch.
  # Growth and elapsed time arrive after the first bounded block is written.
  import hashlib, time
  from contextlib import contextmanager
  for fault in ['none','growth','deadline']:
   with self.subTest(fault=fault),tempfile.TemporaryDirectory() as tmp:
    artifact=Path(tmp)/'formal.zip'
    bodies={'polaris.exe':b'a'*(65536+17),'polaris-helper.exe':b'ordinary Helper'}
    with zipfile.ZipFile(artifact,'w') as archive:
     for name,body in bodies.items():archive.writestr(name,body)
    frozen=artifact.read_bytes();state={'written':0,'reads':0}
    original_open=Path.open;original_exists=Path.exists
    class ObservedStream:
     def __init__(self,stream):self.stream=stream
     def __getattr__(self,name):return getattr(self.stream,name)
     def read(self,size):
      block=self.stream.read(size);state['reads']+=1
      if fault=='growth' and state['reads']==1:
       with original_open(artifact,'ab') as writer:writer.write(b'g'*65536)
      return block
     def write(self,block):
      written=self.stream.write(block);state['written']+=written
      return written
    @contextmanager
    def observed_open(path,mode='r',*args,**kwargs):
     with original_open(path,mode,*args,**kwargs) as stream:
      if (path==artifact and mode=='rb') or (path.name=='carrier.zip' and mode=='xb'):
       yield ObservedStream(stream)
      else:yield stream
    def exists(path):return False if path.parts[:2]==('/','proc') else original_exists(path)
    def clock():return 181 if fault=='deadline' and state['written'] else 0
    with patch.object(Path,'exists',exists),patch.object(Path,'open',observed_open),patch.object(time,'monotonic',clock):
     if fault=='none':
      result=package.reject_local_selection(artifact)
      self.assertEqual(result['artifactBytes'],len(frozen))
      self.assertEqual(result['artifactSha256'],hashlib.sha256(frozen).hexdigest())
      self.assertEqual({row['member']:row['sha256'] for row in result['checkedExecutables']},
                       {name:hashlib.sha256(body).hexdigest() for name,body in bodies.items()})
     else:
      message='snapshot byte budget exceeded' if fault=='growth' else 'inspection deadline exceeded'
      with self.assertRaisesRegex(ValueError,message):package.reject_local_selection(artifact)
    self.assertEqual(state['written'],len(frozen) if fault=='none' else 65536)
    self.assertLessEqual(state['written'],len(frozen))
    if fault!='none':self.assertEqual(state['reads'],2)
 def test_zip_alias_and_expansion_budget_fail_before_payload(self):
  with tempfile.TemporaryDirectory() as tmp:
   for name in ['../polaris.exe','polaris.exe/../polaris.exe','Polaris.exe.']:
    archive=Path(tmp)/'bad.zip'
    with zipfile.ZipFile(archive,'w') as z:z.writestr(name,b'body')
    with self.subTest(name=name),self.assertRaises(ValueError):package.reject_local_selection(archive)
 @unittest.skipUnless(sys.platform=='linux','real Linux container tooling integration')
 def test_actual_deb_and_appimage_accept_formal_and_reject_local(self):
  self.assertIsNotNone(shutil.which('dpkg-deb'));self.assertIsNotNone(shutil.which('mksquashfs'))
  with tempfile.TemporaryDirectory() as tmp:
   root=Path(tmp)
   for local in [False,True]:
    stage=root/('local' if local else 'formal');self.stage(stage,local)
    control=stage/'DEBIAN';control.mkdir();(control/'control').write_text('Package: polaris-control\nVersion: 1.0\nArchitecture: amd64\nMaintainer: Polaris <test@example.invalid>\nDescription: inert source regression\n')
    deb=root/(stage.name+'.deb');subprocess.run(['dpkg-deb','--build','--root-owner-group',str(stage),str(deb)],check=True,capture_output=True)
    image=root/(stage.name+'.AppImage');subprocess.run(['mksquashfs',str(stage),str(image),'-noappend','-processors','1','-quiet'],check=True,capture_output=True)
    for asset in [deb,image]:
     with self.subTest(asset=asset.name):
      if local:
       with self.assertRaisesRegex(ValueError,'LocalDevelopmentSelection executable'):package.reject_local_selection(asset)
      else:self.assertTrue(package.reject_local_selection(asset)['checkedExecutables'])
 @unittest.skipUnless(sys.platform=='linux','POSIX owned executable backend control, not native NSIS/DMG evidence')
 def test_nsis_and_dmg_reader_checks_actual_stdout_and_waits_without_install(self):
  # Controlled 7-Zip backend is a source control, not real NSIS/DMG acceptance.
  from unittest.mock import patch
  with tempfile.TemporaryDirectory() as tmp:
   root=Path(tmp);tool=root/'7zz'
   tool.write_text('#!'+sys.executable+'\nimport sys\nfrom pathlib import Path\na=sys.argv\nif a[1]=="i":print("7-Zip 24.09\\nNsis");sys.exit(0)\np=Path(a[-1]) if a[1]=="l" else Path(a[-2])\nif a[1]=="l":\n print("Type = "+("Nsis" if p.read_bytes().startswith(b"Nsis") else "HFS")+"\\n----------\\nPath = polaris.exe\\nSize = "+str(len(p.read_bytes()))+"\\n\\nPath = polaris-helper.exe\\nSize = 6")\nelse:sys.stdout.buffer.write(p.read_bytes() if a[-1]=="polaris.exe" else b"Helper")\n');tool.chmod(0o700)
   for extension in ['.exe','.dmg']:
    for local in [False,True]:
     artifact=root/('carrier'+extension);artifact.write_bytes((b'Nsis' if extension=='.exe' else b'HFS')+(self.marker if local else b'App'))
     with self.subTest(extension=extension,local=local),patch.dict(os.environ,{'PATH':str(root)+os.pathsep+os.environ['PATH']}):
      if local:
       with self.assertRaisesRegex(ValueError,'LocalDevelopmentSelection executable'):package.reject_local_selection(artifact)
      else:self.assertEqual(len(package.reject_local_selection(artifact)['checkedExecutables']),2)
if __name__=='__main__':unittest.main()
