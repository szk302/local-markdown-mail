//! `name` (§3.2) and `data_dir` (§4.1) in configuration files.
mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use common::*;

/// A command run in `dir` without LMM_DATA_DIR (and without LMM_NAME).
fn cmd_in(e: &Env, dir: &Path) -> Command {
    let mut c = e.cmd(None);
    c.env_remove("LMM_DATA_DIR").current_dir(dir);
    c
}

fn run_in(e: &Env, dir: &Path, args: &[&str]) -> Output {
    cmd_in(e, dir).args(args).output().unwrap()
}

fn ok_in(e: &Env, dir: &Path, args: &[&str]) -> String {
    let o = run_in(e, dir, args);
    assert!(o.status.success(), "lmm {args:?} in {dir:?}: {}", show(&o));
    String::from_utf8(o.stdout).unwrap()
}

/// The scenario that motivated both keys: one agent per worktree, the
/// address in each worktree's `.lmm.local.yml`, and a shared data directory
/// outside the worktrees. No LMM_NAME or LMM_DATA_DIR anywhere.
#[test]
fn worktrees_with_name_and_data_dir() {
    let e = Env::new();
    let shared = e.work.join("common/lmm");
    let wt_a = e.work.join("wt-a");
    let wt_b = e.work.join("wt-b");
    for (wt, name) in [(&wt_a, "alpha"), (&wt_b, "beta")] {
        fs::create_dir_all(wt.join("sub")).unwrap();
        fs::write(
            wt.join(".lmm.local.yml"),
            format!(
                "root: true\nname: {name}\ndata_dir: \"{}\"\n",
                shared.display()
            ),
        )
        .unwrap();
        ok_in(&e, wt, &["box", "create", name]);
    }
    assert!(shared.join("boxes/alpha").is_dir() && shared.join("boxes/beta").is_dir());
    assert!(!e.data_dir.exists());

    // Send from a subdirectory of wt-a.
    let d = e.msg_dir("wt-a/sub/msg", "hello\n", &[]);
    ok_in(
        &e,
        &wt_a.join("sub"),
        &["post", "-t", "beta", d.to_str().unwrap()],
    );
    // Receive in wt-b: the default receive directory uses {name}.
    let out = ok_in(&e, &wt_b, &["receive"]);
    let got = Path::new(out.trim_end());
    assert!(got.starts_with(wt_b.join(".lmm-inbox/beta")), "{out}");
    let index = fs::read_to_string(got.join("index.md")).unwrap();
    assert!(index.contains("lmm_from: alpha\n"), "{index}");

    // LMM_NAME and LMM_DATA_DIR still win.
    let o = cmd_in(&e, &wt_b)
        .env("LMM_NAME", "gamma")
        .env("LMM_DATA_DIR", &e.data_dir)
        .args(["config", "show", "--origin"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("name: gamma  # LMM_NAME\n"), "{out}");
    assert!(
        out.contains(&format!(
            "data_dir: {}  # LMM_DATA_DIR\n",
            e.data_dir.display()
        )),
        "{out}"
    );
    let local = wt_b.join(".lmm.local.yml").display().to_string();
    let out = ok_in(&e, &wt_b, &["config", "show", "--origin"]);
    assert!(out.contains(&format!("name: beta  # {local}\n")), "{out}");
    assert!(
        out.contains(&format!("data_dir: {}  # {local}\n", shared.display())),
        "{out}"
    );
}

/// Worktrees inside the main checkout: each worktree's `.lmm.local.yml`
/// cancels the committed `root: true` with `root: false`, so the data
/// directory written once in the main checkout's `.lmm.local.yml` (relative
/// to it with `{config_dir}`) is shared. No absolute paths.
#[test]
fn nested_worktrees_share_data_dir_without_absolute_paths() {
    let e = Env::new();
    let repo = e.work.join("repo");
    let wt = repo.join("feat-foo");
    let committed = "root: true\nreceive:\n  dir: \"{config_dir}/.lmm-inbox/{name}\"\n";
    for d in [&repo, &wt] {
        fs::create_dir_all(d.join("sub")).unwrap();
        // The committed file: every worktree has its own copy.
        fs::write(d.join(".lmm.yml"), committed).unwrap();
    }
    fs::write(
        repo.join(".lmm.local.yml"),
        "name: root\ndata_dir: \"{config_dir}/.git/lmm\"\n",
    )
    .unwrap();
    fs::write(wt.join(".lmm.local.yml"), "root: false\nname: coder\n").unwrap();
    let data = repo.join(".git/lmm");

    ok_in(&e, &repo, &["box", "create", "root"]);
    ok_in(&e, &wt.join("sub"), &["box", "create", "coder"]);
    assert!(data.join("boxes/root").is_dir() && data.join("boxes/coder").is_dir());

    let d = e.msg_dir("repo/feat-foo/sub/msg", "hi\n", &[]);
    ok_in(
        &e,
        &wt.join("sub"),
        &["post", "-t", "root", d.to_str().unwrap()],
    );
    let out = ok_in(&e, &repo.join("sub"), &["receive"]);
    let got = Path::new(out.trim_end());
    // The receive directory comes from the nearest copy of `.lmm.yml`.
    assert!(got.starts_with(repo.join(".lmm-inbox/root")), "{out}");
    let index = fs::read_to_string(got.join("index.md")).unwrap();
    assert!(index.contains("lmm_from: coder\n"), "{index}");

    // The worktree reads the main checkout's files, after its own.
    let out = ok_in(&e, &wt, &["config", "show", "--origin"]);
    let files: Vec<_> = [
        wt.join(".lmm.local.yml"),
        wt.join(".lmm.yml"),
        repo.join(".lmm.local.yml"),
        repo.join(".lmm.yml"),
    ]
    .iter()
    .map(|p| format!("#   {}\n", p.display()))
    .collect();
    assert!(out.contains(&files.concat()), "{out}");
    assert!(
        out.contains(&format!(
            "  dir: \"{{config_dir}}/.lmm-inbox/{{name}}\"  # {}\n",
            wt.join(".lmm.yml").display()
        )),
        "{out}"
    );
    // Without its own name, a worktree would take the main checkout's.
    fs::write(wt.join(".lmm.local.yml"), "root: false\n").unwrap();
    assert!(ok_in(&e, &wt, &["config", "show"]).contains("name: root\n"));
}

/// `{root_dir}` is where the search stopped, whichever file the value is
/// written in: a committed `data_dir: "{root_dir}/.lmm"` points at the
/// project root from the main checkout and from worktrees inside it that
/// read on with `root: false`.
#[test]
fn root_dir_points_at_project_root() {
    let e = Env::new();
    let repo = e.work.join("repo");
    let wt = repo.join("feat-foo");
    let committed = "root: true\ndata_dir: \"{root_dir}/.lmm\"\n";
    for d in [&repo, &wt] {
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join(".lmm.yml"), committed).unwrap();
    }
    fs::write(repo.join(".lmm.local.yml"), "name: root\n").unwrap();
    fs::write(wt.join(".lmm.local.yml"), "root: false\nname: coder\n").unwrap();
    let data = repo.join(".lmm");

    ok_in(&e, &repo.join("sub"), &["box", "create", "root"]);
    ok_in(&e, &wt.join("sub"), &["box", "create", "coder"]);
    assert!(data.join("boxes/root").is_dir() && data.join("boxes/coder").is_dir());
    let d = e.msg_dir("repo/feat-foo/sub/msg", "hi\n", &[]);
    ok_in(
        &e,
        &wt.join("sub"),
        &["post", "-t", "root", d.to_str().unwrap()],
    );
    let out = ok_in(&e, &repo, &["receive"]);
    let index = fs::read_to_string(Path::new(out.trim_end()).join("index.md")).unwrap();
    assert!(index.contains("lmm_from: coder\n"), "{index}");

    let root_line = |p: &Path| format!("# Root directory ({{root_dir}}): {}\n", p.display());
    let out = ok_in(&e, &wt.join("sub"), &["config", "show"]);
    assert!(out.contains(&root_line(&repo)), "{out}");

    // Stopping at the worktree (its committed root: true), it is the worktree.
    fs::write(wt.join(".lmm.local.yml"), "name: coder\n").unwrap();
    let out = ok_in(&e, &wt.join("sub"), &["config", "show"]);
    assert!(out.contains(&root_line(&wt)), "{out}");

    // No root: true anywhere: an error once the value is used.
    let lone = e.tmp.path().join("lone");
    fs::create_dir(&lone).unwrap();
    fs::write(
        lone.join(".lmm.local.yml"),
        "draft:\n  dir: \"{root_dir}/d\"\n",
    )
    .unwrap();
    let out = ok_in(&e, &lone, &["config", "show"]);
    assert!(
        out.contains("# Root directory ({root_dir}): (none)\n"),
        "{out}"
    );
    let o = run_in(&e, &lone, &["draft", "x"]);
    assert_eq!(
        code(&o),
        0,
        "an explicit DIR does not need it: {}",
        show(&o)
    );
    let o = run_in(&e, &lone, &["draft"]);
    assert_eq!(code(&o), 2, "{}", show(&o));
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("requires a directory config with root: true")
    );
}

#[test]
fn name_only_in_personal_config() {
    let e = Env::new();
    e.create_box("alpha");
    e.create_box("beta");
    let d = e.msg_dir("msg", "hi\n", &[]);
    let post = [&["post", "-t", "beta"][..], &[d.to_str().unwrap()]].concat();

    // Shared config: ignored with a warning.
    fs::write(e.work.join(".lmm.yml"), "root: true\nname: alpha\n").unwrap();
    let o = e.run(None, &post);
    assert_eq!(code(&o), 2, "{}", show(&o));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("name is only valid in .lmm.local.yml"),
        "{err}"
    );
    assert!(err.contains("your address is not set"), "{err}");
    fs::write(e.work.join(".lmm.yml"), "root: true\n").unwrap();

    // Global config: ignored with a warning.
    fs::write(e.data_dir.join("config.yml"), "name: alpha\n").unwrap();
    let o = e.run(None, &post);
    assert_eq!(code(&o), 2, "{}", show(&o));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("name is only valid in .lmm.local.yml"),
        "{err}"
    );
    fs::remove_file(e.data_dir.join("config.yml")).unwrap();

    // Personal config: used.
    fs::write(e.work.join(".lmm.local.yml"), "name: alpha\n").unwrap();
    e.ok(None, &post);
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);

    // Invalid values are config errors.
    for c in [
        "name: Alpha\n",
        "name: lmm\n",
        "name: 1\n",
        "name: \"{cwd}\"\n",
    ] {
        fs::write(e.work.join(".lmm.local.yml"), c).unwrap();
        let o = e.run(None, &["config", "show"]);
        assert_eq!(code(&o), 2, "{c}: {}", show(&o));
    }
}

#[test]
fn nearest_name_wins() {
    let e = Env::new();
    e.create_box("alpha");
    e.create_box("beta");
    fs::write(e.work.join(".lmm.local.yml"), "name: alpha\n").unwrap();
    let inner = e.work.join("inner");
    fs::create_dir(&inner).unwrap();
    fs::write(inner.join(".lmm.local.yml"), "name: beta\n").unwrap();
    let deeper = inner.join("x/y");
    fs::create_dir_all(&deeper).unwrap();
    let o = e
        .cmd(None)
        .current_dir(&deeper)
        .args(["config", "show"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("name: beta\n"));
    // Outside `inner`, the outer name applies.
    assert!(e.ok(None, &["config", "show"]).contains("name: alpha\n"));
}

#[test]
fn hook_participates_with_name_from_config() {
    let e = Env::new();
    e.create_box("alpha");
    e.create_box("beta");
    let d = e.msg_dir("msg", "hi\n", &[]);
    e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);

    // No address: not participating.
    let o = e.run(None, &["hook"]);
    assert_eq!(code(&o), 0, "{}", show(&o));
    assert!(o.stdout.is_empty() && o.stderr.is_empty(), "{}", show(&o));
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);

    // The address from the personal config makes it participate, without
    // HOME when the data directory comes from the config too.
    fs::write(
        e.work.join(".lmm.local.yml"),
        format!("name: beta\ndata_dir: \"{}\"\n", e.data_dir.display()),
    )
    .unwrap();
    let o = cmd_in(&e, &e.work)
        .env_remove("HOME")
        .arg("hook")
        .output()
        .unwrap();
    assert_eq!(code(&o), 0, "{}", show(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.starts_with("You have 1 new message"), "{out}");
    assert!(out.contains(".lmm-inbox/beta/"), "{out}");
    assert!(entries(&e.new_dir("beta")).is_empty());

    // Config errors are reported even when not participating (1, not 2).
    fs::write(e.work.join(".lmm.local.yml"), "hook:\n  timeout: 0\n").unwrap();
    let o = e.run(None, &["hook"]);
    assert_eq!(code(&o), 1, "{}", show(&o));
}

#[test]
fn data_dir_from_config() {
    let e = Env::new();
    let proj = e.work.join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(
        proj.join(".lmm.yml"),
        "root: true\ndata_dir: \"{config_dir}/.lmm-data\"\n",
    )
    .unwrap();
    let data = proj.join(".lmm-data");
    ok_in(&e, &sub, &["box", "create", "alpha"]);
    assert!(data.join("boxes/alpha").is_dir());
    assert_eq!(
        ok_in(&e, &proj, &["box", "list"]),
        "alpha\tnew=0\tfailed=0\n"
    );

    // The global config is read from that data directory, and only fills
    // what the directory configs and the environment left unset.
    fs::write(
        data.join("config.yml"),
        "hook:\n  timeout: 9\n  cmd: from-global\npost:\n  meta:\n    a: g\n",
    )
    .unwrap();
    fs::write(sub.join(".lmm.yml"), "post:\n  meta:\n    b: d\n    a: d\n").unwrap();
    let g = data.join("config.yml").display().to_string();
    let top = proj.join(".lmm.yml").display().to_string();
    let o = cmd_in(&e, &sub)
        .env("LMM_HOOK_CMD", "from-env")
        .args(["config", "show", "--origin"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", show(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains(&format!("#   {g}\n")), "{out}");
    assert!(
        out.contains(&format!(
            "data_dir: \"{{config_dir}}/.lmm-data\"  # {top}\n"
        )),
        "{out}"
    );
    assert!(out.contains(&format!("  timeout: 9  # {g}\n")), "{out}");
    assert!(out.contains("  cmd: from-env  # LMM_HOOK_CMD\n"), "{out}");
    assert!(out.contains("    a: d  # "), "{out}");

    // Receive and draft directories must stay outside it.
    let o = cmd_in(&e, &proj)
        .env("LMM_NAME", "alpha")
        .args(["receive", ".lmm-data/x"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 2, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("inside the data directory"));

    // Without the config, the default (LMM_DATA_DIR unset: ~/.lmm).
    let out = ok_in(&e, &e.work, &["config", "show", "--origin"]);
    assert!(out.contains("data_dir: ~/.lmm  # (default)\n"), "{out}");
    let out = ok_in(&e, &e.work, &["box", "list"]);
    assert_eq!(out, "");
    assert!(!e.user_home.join(".lmm/boxes/alpha").exists());
}

#[test]
fn data_dir_errors() {
    let e = Env::new();
    for c in [
        "data_dir: rel/dir\n",
        "data_dir: \"{name}/x\"\n",
        "data_dir: \"/x/{name}\"\n",
        "data_dir: \"{nope}\"\n",
        "data_dir: 1\n",
    ] {
        fs::write(e.work.join(".lmm.local.yml"), c).unwrap();
        let o = run_in(&e, &e.work, &["box", "list"]);
        assert_eq!(code(&o), 2, "{c}: {}", show(&o));
    }
    for c in [
        "data_dir: \"~/d\"\n",
        "data_dir: \"{cwd}/d\"\n",
        "data_dir: \"{config_dir}/{{x}\"\n",
    ] {
        fs::write(e.work.join(".lmm.local.yml"), c).unwrap();
        let o = run_in(&e, &e.work, &["box", "list"]);
        assert_eq!(code(&o), 0, "{c}: {}", show(&o));
    }
    fs::remove_file(e.work.join(".lmm.local.yml")).unwrap();

    // Not in the global config (its location depends on the data directory).
    fs::create_dir_all(&e.data_dir).unwrap();
    fs::write(e.data_dir.join("config.yml"), "data_dir: /elsewhere\n").unwrap();
    let o = e.run(None, &["config", "show", "--origin"]);
    assert!(o.status.success(), "{}", show(&o));
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("data_dir is not valid in the global config")
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("# LMM_DATA_DIR\n"));
}
