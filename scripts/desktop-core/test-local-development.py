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

if __name__=='__main__':unittest.main()
