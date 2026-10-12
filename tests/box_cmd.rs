mod common;

use std::fs;

use common::*;

#[test]
fn help_and_usage_errors() {
    let e = Env::new();
    let o = e.run(None, &["--help"]);
    assert_eq!(code(&o), 0);
    let help = String::from_utf8_lossy(&o.stdout);
    for c in [
        "post", "draft", "receive", "wait", "show", "hook", "box", "config",
    ] {
        assert!(help.contains(c), "{c} missing in help");
    }
    assert_eq!(code(&e.run(None, &["--nope"])), 2);
    assert_eq!(code(&e.run(None, &["post", "--nope", "x"])), 2);
    // `lmm hook` never returns 2 itself.
    assert_eq!(code(&e.run(None, &["hook", "--nope"])), 1);
}

#[test]
fn box_create_list_remove() {
    let e = Env::new();
    e.create_box("alpha");
    let b = e.data_dir.join("boxes/alpha");
    for d in [
        "inbox/.tmp",
        "inbox/new",
        "inbox/cur",
        "inbox/failed",
        "sent",
    ] {
        assert!(b.join(d).is_dir(), "{d}");
    }
    let o = e.run(None, &["box", "create", "alpha"]);
    assert_eq!(code(&o), 1, "{}", show(&o));
    e.ok(None, &["box", "create", "alpha", "--exist-ok"]);
    e.create_box("beta");
    let list = e.ok(None, &["box", "list"]);
    assert_eq!(list, "alpha\tnew=0\tfailed=0\nbeta\tnew=0\tfailed=0\n");
    e.ok(None, &["box", "remove", "beta"]);
    assert!(!e.data_dir.join("boxes/beta").exists());
    assert!(
        e.data_dir.join("locks/box/beta.lock").exists(),
        "lock files are kept"
    );
    assert_eq!(e.ok(None, &["box", "list"]), "alpha\tnew=0\tfailed=0\n");
    assert_eq!(code(&e.run(None, &["box", "remove", "beta"])), 1);
}

#[test]
fn box_invalid_addresses() {
    let e = Env::new();
    for a in ["..", ".", "Alpha", "lmm", "../x", "a/b", ""] {
        assert_eq!(code(&e.run(None, &["box", "create", a])), 2, "{a}");
        assert_eq!(code(&e.run(None, &["box", "remove", a])), 2, "{a}");
    }
    assert!(!e.data_dir.join("boxes/..").join("x").exists());
}

#[test]
fn box_create_cleans_leftovers() {
    let e = Env::new();
    e.create_box("alpha");
    let creating = e.data_dir.join("boxes/.creating/beta/junk");
    let removing = e.data_dir.join("boxes/.removing/beta/junk");
    fs::create_dir_all(&creating).unwrap();
    fs::create_dir_all(&removing).unwrap();
    e.create_box("beta");
    assert!(!creating.exists() && !removing.exists());
    let list = e.ok(None, &["box", "list"]);
    assert!(!list.contains(".creating"), "{list}");
}

#[test]
fn box_remove_refuses_pending_and_live_sends() {
    let e = Env::new();
    e.create_box("alpha");
    e.create_box("beta");
    let d = e.msg_dir("m", "hi\n", &[]);
    e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);
    let o = e.run(None, &["box", "remove", "beta"]);
    assert_eq!(code(&o), 1, "{}", show(&o));
    assert!(e.data_dir.join("boxes/beta").exists());

    // Abandoned work directory (no lock held) is removed together.
    e.create_box("gamma");
    let wd = e.tmp_dir("gamma").join("0000-abandoned");
    fs::create_dir_all(wd.join("msg")).unwrap();
    fs::write(wd.join("lock"), "").unwrap();
    e.ok(None, &["box", "remove", "gamma"]);

    // A live work directory (lock held by another process) blocks removal.
    e.create_box("delta");
    let wd = e.tmp_dir("delta").join("0000-live");
    fs::create_dir_all(&wd).unwrap();
    let lockf = fs::File::create(wd.join("lock")).unwrap();
    lockf.lock().unwrap();
    let o = e.run(None, &["box", "remove", "delta"]);
    assert_eq!(code(&o), 1, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("in progress"));
    e.ok(None, &["box", "remove", "delta", "--force"]);
    assert!(!e.data_dir.join("boxes/delta").exists());
    drop(lockf);

    e.ok(None, &["box", "remove", "beta", "--force"]);
}
