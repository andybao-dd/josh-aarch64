  $ . ${TESTDIR}/setup_test_env.sh
  $ cd ${TESTTMP}

  $ git clone -q http://localhost:8001/real_repo.git 1> /dev/null
  warning: You appear to have cloned an empty repository.
  $ cd real_repo

  $ mkdir sub1
  $ echo contents1 > sub1/file1
  $ git add sub1
  $ git commit -m "add file1" 1> /dev/null
  $ git push 1> /dev/null
  To http://localhost:8001/real_repo.git
   * [new branch]      master -> master

  $ cd ${TESTTMP}

  $ git clone -q http://localhost:8002/real_repo.git:/sub1.git
  $ cd sub1

  $ echo contents2 > file2
  $ git add file2
  $ git commit -m "add file2" 1> /dev/null
  $ git push origin HEAD:refs/heads/new_branch 2>&1 >/dev/null | sed -e 's/[ ]*$//g'
  remote: josh-proxy
  remote: response from upstream:
  remote: Reference "refs/heads/new_branch" does not exist on remote.
  remote: If you want to create it, pass "-o base=refs/heads/<branchname>"
  remote: to specify a base branch/reference.
  remote:
  remote:
  remote:
  remote: error: hook declined to update refs/heads/new_branch
  To http://localhost:8002/real_repo.git:/sub1.git
   ! [remote rejected] HEAD -> new_branch (hook declined)
  error: failed to push some refs to 'http://localhost:8002/real_repo.git:/sub1.git'

  $ git push -o base=refs/heads/master origin HEAD:refs/heads/new_branch 2>&1 >/dev/null | sed -e 's/[ ]*$//g'
  remote: josh-proxy
  remote: response from upstream:
  remote: To http://localhost:8001/real_repo.git
  remote:  * [new branch]      JOSH_PUSH -> new_branch
  remote:
  remote:
  To http://localhost:8002/real_repo.git:/sub1.git
   * [new branch]      HEAD -> new_branch

$ curl -s http://localhost:8002/flush
Flushed credential cache
  $ git push
  remote: josh-proxy        
  remote: response from upstream:        
  remote: To http://localhost:8001/real_repo.git        
  remote:    bb282e9..81b10fb  JOSH_PUSH -> master        
  remote: 
  remote: 
  To http://localhost:8002/real_repo.git:/sub1.git
     0b4cf6c..d8388f5  master -> master

  $ cd ${TESTTMP}/real_repo
  $ git pull --rebase
  From http://localhost:8001/real_repo
     bb282e9..81b10fb  master     -> origin/master
   * [new branch]      new_branch -> origin/new_branch
  Updating bb282e9..81b10fb
  Fast-forward
   sub1/file2 | 1 +
   1 file changed, 1 insertion(+)
   create mode 100644 sub1/file2

  $ tree
  .
  `-- sub1
      |-- file1
      `-- file2
  
  1 directory, 2 files

  $ cat sub1/file2
  contents2

Make sure all temporary namespace got removed
  $ tree ${TESTTMP}/remote/scratch/real_repo.git/refs/ | grep request_
  [1]

  $ bash ${TESTDIR}/destroy_test_env.sh
  "real_repo.git" = [':/sub1']
  refs
  |-- heads
  |-- josh
  |   |-- filtered
  |   |   `-- real_repo.git
  |   |       `-- %3A%2Fsub1
  |   |           `-- HEAD
  |   `-- upstream
  |       `-- real_repo.git
  |           |-- HEAD
  |           `-- refs
  |               `-- heads
  |                   |-- master
  |                   `-- new_branch
  |-- namespaces
  `-- tags
  
  11 directories, 4 files

$ cat ${TESTTMP}/josh-proxy.out
