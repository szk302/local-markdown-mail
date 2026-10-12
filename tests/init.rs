//! `lmm init` (§10.8).
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::*;

/// Run in `dir` without LMM_DATA_DIR, so that `data_dir` decides.
fn run_in(e: &Env, dir: &Path, args: &[&str]) -> Output {
    e.cmd(None)
        .env_remove("LMM_DATA_DIR")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

fn ok_in(e: &Env, dir: &Path, args: &[&str]) -> Vec<PathBuf> {
    let o = run_in(e, dir, args);
    assert!(o.status.success(), "lmm {args:?} in {dir:?}: {}", show(&o));
    String::from_utf8(o.stdout)
        .unwrap()
        .lines()
        .map(PathBuf::from)
        .collect()
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

#[test]
fn root_then_join() {
    let e = Env::new();
    let proj = e.work.join("proj");
    fs::create_dir(&proj).unwrap();
    fs::write(proj.join(".gitignore"), "target\n.lmm/").unwrap();

    // The project root.
    let out = ok_in(&e, &proj, &["init", "--root", "--name", "root"]);
    assert_eq!(
        out,
        [
            proj.join(".lmm.yml"),
            proj.join(".gitignore"),
            proj.join(".lmm.local.yml")
        ]
    );
    assert!(
        read(&proj.join(".lmm.yml")).starts_with("root: true\ndata_dir: \"{root_dir}/.lmm\"\n")
    );
    assert_eq!(
        read(&proj.join(".gitignore")),
        "target\n.lmm/\n.lmm.local.yml\n"
    );
    assert_eq!(read(&proj.join(".lmm.local.yml")), "name: root\n");
    assert!(proj.join(".lmm/boxes/root").is_dir());

    // Again: nothing changes.
    assert!(ok_in(&e, &proj, &["init", "--root", "--name", "root"]).is_empty());

    // A worktree inside it, with its copy of the committed files.
    let wt = proj.join("feat");
    fs::create_dir(&wt).unwrap();
    for f in [".lmm.yml", ".gitignore"] {
        fs::copy(proj.join(f), wt.join(f)).unwrap();
    }
    let gitignore = read(&wt.join(".gitignore"));
    let out = ok_in(&e, &wt, &["init", "--name", "coder"]);
    assert_eq!(out, [wt.join(".lmm.local.yml")]);
    assert_eq!(
        read(&wt.join(".lmm.local.yml")),
        "root: false\nname: coder\n"
    );
    // The committed copies are left alone.
    assert_eq!(read(&wt.join(".gitignore")), gitignore);
    // The mailbox is in the project's data directory.
    assert!(proj.join(".lmm/boxes/coder").is_dir());
    assert!(!wt.join(".lmm").exists());
    assert!(ok_in(&e, &wt, &["init", "--name", "coder"]).is_empty());

    // They can talk without any environment variable.
    let sub = wt.join("src");
    fs::create_dir(&sub).unwrap();
    let d = e.msg_dir("proj/feat/src/msg", "hi\n", &[]);
    ok_in(&e, &sub, &["post", "-t", "root", d.to_str().unwrap()]);
    let got = ok_in(&e, &proj, &["receive"]);
    assert!(got[0].starts_with(proj.join(".lmm-inbox/root")), "{got:?}");
    assert!(read(&got[0].join("index.md")).contains("lmm_from: coder\n"));

    // DIR from elsewhere.
    let other = proj.join("other");
    fs::create_dir(&other).unwrap();
    let out = ok_in(&e, &proj, &["init", "--name", "other", "other"]);
    assert_eq!(out, [other.join(".lmm.local.yml")]);
    assert!(proj.join(".lmm/boxes/other").is_dir());
}

#[test]
fn init_errors_change_nothing() {
    let e = Env::new();
    let proj = e.work.join("proj");
    fs::create_dir(&proj).unwrap();
    ok_in(&e, &proj, &["init", "--root"]);
    assert!(!proj.join(".lmm.local.yml").exists());
    assert!(!proj.join(".lmm").exists());

    // A different personal config is not overwritten.
    let wt = proj.join("wt");
    fs::create_dir(&wt).unwrap();
    fs::write(wt.join(".lmm.local.yml"), "name: other\n").unwrap();
    let o = run_in(&e, &wt, &["init", "--name", "coder"]);
    assert_eq!(code(&o), 1, "{}", show(&o));
    assert_eq!(read(&wt.join(".lmm.local.yml")), "name: other\n");
    assert!(!proj.join(".lmm/boxes/coder").exists());

    // Usage errors.
    let fresh = proj.join("fresh");
    fs::create_dir(&fresh).unwrap();
    for args in [
        &["init"][..],
        &["init", "--name", "Bad"],
        &["init", "--name", "lmm"],
        &["init", "--name", "x", "missing"],
    ] {
        let o = run_in(&e, &fresh, args);
        assert_eq!(code(&o), 2, "{args:?}: {}", show(&o));
    }
    assert_eq!(entries(&fresh), Vec::<String>::new());

    // Joining needs a project above.
    let lone = e.tmp.path().join("lone");
    fs::create_dir(&lone).unwrap();
    let o = run_in(&e, &lone, &["init", "--name", "coder"]);
    assert_eq!(code(&o), 2, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("lmm init --root"));
    assert_eq!(entries(&lone), Vec::<String>::new());
}
