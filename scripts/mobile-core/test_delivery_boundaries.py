"""Local regression fixtures; no Cargo, Xcode, VM/App or remote operations."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).resolve().parent))
import fork_input
import consume
import apple_rust
class Boundaries(unittest.TestCase):
 def test_git_overrides_cannot_mask_actual_dirty_root(self):
  with tempfile.TemporaryDirectory() as tmp:
   actual,other=Path(tmp)/'actual',Path(tmp)/'other'
   for repo in [actual,other]:
    repo.mkdir();subprocess.run(['git','init','-q',str(repo)],check=True,env=fork_input.git_environment())
    (repo/'source.rs').write_text('before')
    subprocess.run(['git','-C',str(repo),'add','.'],check=True,env=fork_input.git_environment())
    subprocess.run(['git','-C',str(repo),'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-qm','fixture'],check=True,env=fork_input.git_environment())
   (actual/'source.rs').write_text('changed')
   with patch.dict(os.environ,{'GIT_DIR':str(other/'.git'),'GIT_WORK_TREE':str(other),'GIT_INDEX_FILE':str(other/'.git/index'),'GIT_CONFIG_COUNT':'1','GIT_CONFIG_KEY_0':'status.showUntrackedFiles','GIT_CONFIG_VALUE_0':'no'}),patch.object(consume,'ROOT',actual):
    self.assertIn('source.rs',fork_input.git(actual,'status','--porcelain'))
    with self.assertRaisesRegex(RuntimeError,'not frozen'):consume.app_identity()
   nested=actual/'nested';nested.mkdir()
   with self.assertRaisesRegex(RuntimeError,'root'):fork_input.require_git_root(nested)
 def test_stage_dangling_existing_and_race(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=Path(tmp);source=root/'archive';source.write_bytes(b'!<arch>\nfixture');stage=root/'stage';target=root/'outside'
   stage.symlink_to(target)
   with self.assertRaises(RuntimeError):apple_rust.copy_exclusive_stage(source,stage)
   self.assertFalse(target.exists());stage.unlink();target.write_bytes(b'keep');stage.symlink_to(target)
   with self.assertRaises(RuntimeError):apple_rust.copy_exclusive_stage(source,stage)
   self.assertEqual(target.read_bytes(),b'keep');stage.unlink();real_open=os.open
   def race(path,flags,mode):
    Path(path).symlink_to(target);return real_open(path,flags,mode)
   with patch.object(apple_rust.os,'open',side_effect=race):
    with self.assertRaises(FileExistsError):apple_rust.copy_exclusive_stage(source,stage)
   self.assertEqual(target.read_bytes(),b'keep');stage.unlink();apple_rust.copy_exclusive_stage(source,stage);self.assertEqual(stage.read_bytes(),source.read_bytes())
if __name__=='__main__':unittest.main()
