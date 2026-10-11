mod common;

use std::fs;
use std::path::PathBuf;

use common::*;

fn setup() -> Env {
    let e = Env::new();
    e.create_box("alpha");
    e.create_box("beta");
    e
}

fn send(e: &Env, body: &str) -> (String, PathBuf) {
    let d = e.msg_dir(&format!("m-{}", uuid_like()), body, &[("att.txt", b"A")]);
    e.post("alpha", &["-t", "beta", d.to_str().unwrap()])
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    N.fetch_add(1, Ordering::SeqCst).to_string()
}

#[test]
fn receive_extracts_and_moves_to_cur() {
    let e = setup();
    let (_, p1) = send(&e, "one\n");
    let (_, p2) = send(&e, "two\n");
    let out = e.ok(Some("beta"), &["receive", "--all"]);
    let dest = e.work.join(".lmm-inbox/beta");
    let n1 = p1.file_name().unwrap();
    let n2 = p2.file_name().unwrap();
    assert_eq!(
        lines(&out),
        vec![
            dest.join(n1).display().to_string(),
            dest.join(n2).display().to_string()
        ]
    );
    assert_eq!(
        fs::read_to_string(dest.join(n1).join("att.txt")).unwrap(),
        "A"
    );
    assert_eq!(
        fs::read_to_string(e.work.join(".lmm-inbox/.gitignore")).unwrap(),
        "*\n"
    );
    assert!(entries(&e.new_dir("beta")).is_empty());
    assert_eq!(entries(&e.cur_dir("beta")).len(), 2);
    assert!(!entries(&dest).iter().any(|n| n.starts_with('.')));
    // Second run: nothing.
    assert_eq!(e.ok(Some("beta"), &["receive"]), "");
}

/// §7.1: one message per run by default, oldest first.
#[test]
fn receive_defaults_to_one_oldest_message() {
    let e = setup();
    let sent: Vec<_> = (0..3).map(|i| send(&e, &format!("{i}\n")).1).collect();
    let dest = e.work.join(".lmm-inbox/beta");
    for p in &sent {
        let out = e.ok(Some("beta"), &["receive"]);
        assert_eq!(
            lines(&out),
            vec![dest.join(p.file_name().unwrap()).display().to_string()]
        );
    }
    assert_eq!(e.ok(Some("beta"), &["receive"]), "");
    // --all takes everything; --all with --max is a usage error.
    for i in 0..3 {
        send(&e, &format!("again {i}\n"));
    }
    assert_eq!(
        code(&e.run(Some("beta"), &["receive", "--all", "--max", "2"])),
        2
    );
    assert_eq!(lines(&e.ok(Some("beta"), &["receive", "--all"])).len(), 3);
    assert!(entries(&e.new_dir("beta")).is_empty());
}

#[test]
fn receive_max_and_explicit_dir() {
    let e = setup();
    for i in 0..3 {
        send(&e, &format!("{i}\n"));
    }
    let out = e.ok(Some("beta"), &["receive", "--max", "2", "custom"]);
    assert_eq!(lines(&out).len(), 2);
    assert!(lines(&out)[0].starts_with(&e.work.join("custom").display().to_string()));
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);
    assert_eq!(code(&e.run(Some("beta"), &["receive", "--max", "0"])), 2);
    // Env var destination.
    let o = e
        .cmd(Some("beta"))
        .env("LMM_RECEIVE_DIR", e.work.join("fromenv"))
        .arg("receive")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(entries(&e.work.join("fromenv")).len(), 1);
}

#[test]
fn receive_errors() {
    let e = setup();
    assert_eq!(code(&e.run(None, &["receive"])), 2);
    let o = e.run(Some("gamma"), &["receive"]);
    assert_eq!(code(&o), 1);
    assert!(String::from_utf8_lossy(&o.stderr).contains("lmm box create gamma"));
    assert!(!e.home.join("locks/receive/gamma.lock").exists());
    // Destination inside the data directory.
    send(&e, "x\n");
    let inside = e.home.join("boxes/beta/inbox/cur");
    let o = e.run(Some("beta"), &["receive", inside.to_str().unwrap()]);
    assert_eq!(code(&o), 2, "{}", show(&o));
    // Via a symlink to the data directory.
    let link = e.work.join("link");
    std::os::unix::fs::symlink(&e.home, &link).unwrap();
    let o = e.run(Some("beta"), &["receive", "link/x/y"]);
    assert_eq!(code(&o), 2, "{}", show(&o));
    assert!(!e.home.join("x").exists());
    // Newline in destination.
    assert_eq!(code(&e.run(Some("beta"), &["receive", "a\nb"])), 2);
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);
}

#[test]
fn invalid_messages_move_to_failed() {
    let e = setup();
    let (_, good) = send(&e, "good\n");
    // Hand-placed broken messages sorted before the good one.
    let bad1 = e
        .new_dir("beta")
        .join("00000000-000000.000000_x_to_beta_aaaaaaaa");
    fs::create_dir(&bad1).unwrap();
    fs::write(bad1.join("index.md"), "no front matter\n").unwrap();
    let bad2 = e
        .new_dir("beta")
        .join("00000000-000000.000001_x_to_beta_bbbbbbbb");
    fs::create_dir(&bad2).unwrap();
    fs::write(
        bad2.join("index.md"),
        received_index("a1b2c3d4-e5f6-4890-abcd-ef1234567890", "alpha", "beta", ""),
    )
    .unwrap();
    fs::create_dir(bad2.join("sub")).unwrap();
    let o = e.run(Some("beta"), &["receive"]);
    assert_eq!(code(&o), 1, "{}", show(&o));
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert_eq!(lines(&stdout).len(), 1);
    assert!(stdout.contains(good.file_name().unwrap().to_str().unwrap()));
    let failed = entries(&e.failed_dir("beta"));
    assert_eq!(failed.len(), 2, "{failed:?}");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("moved to"), "{err}");
    assert_eq!(
        e.ok(None, &["box", "list"]),
        "alpha\tnew=0\tfailed=0\nbeta\tnew=0\tfailed=2\n"
    );
}

#[test]
fn interrupted_receive_is_detected_as_extracted() {
    let e = setup();
    let (_, p) = send(&e, "x\n");
    let name = p.file_name().unwrap().to_owned();
    let dest = e.work.join("in");
    // Simulate a run that stopped between steps iii and iv.
    fs::create_dir_all(&dest).unwrap();
    let copy = dest.join(&name);
    fs::create_dir(&copy).unwrap();
    fs::copy(p.join("index.md"), copy.join("index.md")).unwrap();
    // A stale temp dir from a crashed copy.
    let mut tmpname = std::ffi::OsString::from(".");
    tmpname.push(&name);
    tmpname.push(".tmp");
    fs::create_dir(dest.join(&tmpname)).unwrap();
    let out = e.ok(Some("beta"), &["receive", "in"]);
    assert_eq!(out.trim_end(), copy.display().to_string());
    assert!(!copy.join("att.txt").exists(), "copy skipped");
    // A different message with the same name gets a suffix.
    let (_, p2) = send(&e, "y\n");
    let name2 = p2.file_name().unwrap().to_owned();
    let other = dest.join(&name2);
    fs::create_dir(&other).unwrap();
    fs::write(other.join("index.md"), "unrelated").unwrap();
    let out = e.ok(Some("beta"), &["receive", "in"]);
    let mut n1 = name2.clone();
    n1.push("_1");
    assert_eq!(out.trim_end(), dest.join(n1).display().to_string());
}

#[test]
fn abandoned_work_dirs_are_cleaned_but_live_ones_kept() {
    let e = setup();
    let tmp = e.tmp_dir("beta");
    let abandoned = tmp.join("abandoned");
    fs::create_dir_all(abandoned.join("msg")).unwrap();
    fs::write(abandoned.join("lock"), "").unwrap();
    let nolock = tmp.join("nolock");
    fs::create_dir_all(&nolock).unwrap();
    let live = tmp.join("live");
    fs::create_dir_all(&live).unwrap();
    let f = fs::File::create(live.join("lock")).unwrap();
    f.lock().unwrap();
    e.ok(Some("beta"), &["receive"]);
    assert_eq!(entries(&tmp), vec!["live".to_string()]);
    drop(f);
    // While a sender holds the prepare lock, cleanup is skipped.
    let prep = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(e.home.join("locks/prepare/beta.lock"))
        .unwrap();
    prep.lock_shared().unwrap();
    e.ok(Some("beta"), &["receive"]);
    assert_eq!(entries(&tmp), vec!["live".to_string()]);
    drop(prep);
    e.ok(Some("beta"), &["receive"]);
    assert!(entries(&tmp).is_empty());
}
