import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT=Path(__file__).resolve().parents[1]/'tools'/'git-sync.py'
HOOK=SCRIPT.parents[1]/'.githooks'
class GitSyncTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(prefix='unifi-git-test-');self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
        self.env={**os.environ,'GIT_CONFIG_GLOBAL':'/dev/null','GIT_CONFIG_NOSYSTEM':'1','GIT_AUTHOR_NAME':'Test','GIT_COMMITTER_NAME':'Test','GIT_AUTHOR_EMAIL':'test@example.invalid','GIT_COMMITTER_EMAIL':'test@example.invalid'}
        self.git(self.root,'init','--bare','remote')
        self.git(self.root,'clone',str(self.root/'remote'),'producer')
        self.producer=self.root/'producer';self.clone=self.root/'clone'
        self.git(self.producer,'checkout','-b','main');self.commit(self.producer,'initial')
        self.git(self.producer,'push','origin','main')
        self.git(self.root/'remote','symbolic-ref','HEAD','refs/heads/main')
        self.git(self.root,'clone',str(self.root/'remote'),str(self.clone))
        self.git(self.clone,'config','core.hooksPath',str(HOOK))
        self.git(self.clone,'config','fetch.prune','true')
    def git(self,cwd,*args):
        return subprocess.run(['git',*args],cwd=cwd,env=self.env,check=True,text=True,capture_output=True).stdout.strip()
    def commit(self,cwd,text):
        (cwd/'file').write_text(text);self.git(cwd,'add','file');self.git(cwd,'commit','-m',text)
        return self.git(cwd,'rev-parse','HEAD')
    def upstream(self):
        new=self.commit(self.producer,'updated');self.git(self.producer,'push','origin','main');return new
    def test_ordinary_fetch_updates_unchecked_main(self):
        self.git(self.clone,'checkout','-b','work');new=self.upstream();self.git(self.clone,'fetch')
        self.assertEqual(self.git(self.clone,'rev-parse','main'),new)
        self.assertNotEqual(self.git(self.clone,'rev-parse','HEAD'),new)
    def test_checked_out_main_in_another_worktree_is_untouched(self):
        self.git(self.clone,'checkout','-b','work');self.git(self.clone,'worktree','add',str(self.root/'other'),'main')
        old=self.git(self.clone,'rev-parse','main');self.upstream();self.git(self.clone,'fetch')
        self.assertEqual(self.git(self.clone,'rev-parse','main'),old)
    def test_diverged_main_is_untouched(self):
        old=self.commit(self.clone,'local change');self.git(self.clone,'checkout','-b','work');self.upstream();self.git(self.clone,'fetch')
        self.assertEqual(self.git(self.clone,'rev-parse','main'),old)
    def test_prunes_deleted_remote_branches(self):
        self.git(self.producer,'push','origin','main:old');self.git(self.clone,'fetch')
        self.git(self.producer,'push','origin','--delete','old');self.git(self.clone,'fetch')
        self.assertNotIn('origin/old',self.git(self.clone,'branch','-r'))
if __name__=='__main__':unittest.main()
