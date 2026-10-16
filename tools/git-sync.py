#!/usr/bin/env python3
"""Prune on fetch and fast-forward main only when it is not checked out."""
import argparse
import os
import subprocess
import sys

def git(*args, check=True):
    return subprocess.run(['git', *args], text=True, capture_output=True, check=check)

def update_main(remote=None):
    remote = remote or git('config', '--get', 'branch.main.remote', check=False).stdout.strip() or 'origin'
    merge = git('config', '--get', 'branch.main.merge', check=False).stdout.strip() or 'refs/heads/main'
    if remote == '.' or not merge.startswith('refs/heads/'):
        return
    target = 'refs/remotes/' + remote + '/' + merge.removeprefix('refs/heads/')
    new = git('rev-parse', '--verify', target, check=False)
    if new.returncode:
        return
    # Check all worktrees, including locked worktrees. Never use --update-head-ok.
    worktrees = git('worktree', 'list', '--porcelain', '-z').stdout.split('\0')
    if 'branch refs/heads/main' in worktrees:
        return
    old = git('rev-parse', '--verify', 'refs/heads/main', check=False)
    if old.returncode:
        # Do not create an untracked local branch automatically.
        return
    old, new = old.stdout.strip(), new.stdout.strip()
    if old == new:
        return
    if git('merge-base', '--is-ancestor', old, new, check=False).returncode:
        print('git-sync: main has local commits or diverged; left unchanged', file=sys.stderr)
        return
    result = git('update-ref', '-m', 'fetch: fast-forward unchecked-out main', 'refs/heads/main', new, old, check=False)
    if result.returncode:
        print('git-sync: main changed concurrently; left unchanged', file=sys.stderr)
    else:
        print('git-sync: fast-forwarded main', file=sys.stderr)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('remote', nargs='?', default=None)
    parser.add_argument('--hook', action='store_true')
    args = parser.parse_args()
    if args.hook:
        # A committed reference-transaction hook also covers ordinary git fetch.
        changes = sys.stdin.read().splitlines()
        if not any(line.split()[-1].startswith('refs/remotes/') and line.split()[1].strip('0') for line in changes if len(line.split()) == 3):
            return
    else:
        result = subprocess.run(['git', 'fetch', '--prune', '--atomic'] + ([args.remote] if args.remote else []))
        if result.returncode:
            raise SystemExit(result.returncode)
    update_main(args.remote)

if __name__ == '__main__':
    try:
        main()
    except (subprocess.CalledProcessError, OSError) as error:
        # A post-commit hook cannot undo a committed ref transaction.
        print('git-sync:', error, file=sys.stderr)
        raise SystemExit(1)
