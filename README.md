# Migrate to Kernel Repo

this is a small tool that helps migrate kernels in model repos into their own kernel repos on the Hugging Face Hub. It clones the model repo, squashes all branches into one commit, and pushes to a new kernel repo.

---

install with

```bash
cargo install --git https://github.com/drbh/migrate-to-kernel-repo
```

run on a repo to migrate it to a kernel repo on the Hugging Face Hub

```bash
migrate-to-kernel-repo kernels-test/relu-tvm-ffi
```

example output
```txt
Migrating 1 repo(s)...

repo:       kernels-test/relu-tvm-ffi
model url:  git@hf.co:kernels-test/relu-tvm-ffi
kernel url: git@hf.co:kernels/kernels-test/relu-tvm-ffi
creating kernel repo on Hub...
cloning model repo...
  $ git clone git@hf.co:kernels-test/relu-tvm-ffi /var/folders/ht/70c4_m411w51qrq__drl8m4r0000gn/T/migrate-kernel-kernels-test--relu-tvm-ffi
    Cloning into '/var/folders/ht/70c4_m411w51qrq__drl8m4r0000gn/T/migrate-kernel-kernels-test--relu-tvm-ffi'...
Filtering content: 100% (6/6), 11.35 MiB | 7.09 MiB/s, done.
fetching LFS objects...
  $ git lfs fetch --all origin
    19 objects found, done.
    Fetching all references...
branches: ["main", "v1"]
squashing branches...
  squashing main
  squashing v1
pushing to kernel repo...
  $ git push --force --all origin
    Uploading LFS objects: 100% (6/6), 12 MB | 0 B/s, done.
    To hf.co:kernels/kernels-test/relu-tvm-ffi
     + ad06d39...29cea97 main -> main (forced update)
     * [new branch]      v1 -> v1
migration complete: https://huggingface.co/kernels-test/relu-tvm-ffi
cleaning up local clone

Results: 1 succeeded, 0 failed
```