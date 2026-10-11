mod common;

use std::collections::HashSet;
use std::fs;
use std::process::Stdio;

use common::*;

fn setup() -> Env {
    let e = Env::new();
    e.create_box("alpha");
    e.create_box("beta");
    e
}

#[test]
fn concurrent_posts_all_delivered_with_unique_names() {
    let e = setup();
    let d = e.msg_dir("m", "x\n", &[("a.bin", &[1u8; 4096])]);
    let n = 16;
    let children: Vec<_> = (0..n)
        .map(|_| {
            e.cmd(Some("alpha"))
                .args(["post", "-t", "beta", d.to_str().unwrap()])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut ids = HashSet::new();
    for c in children {
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success(), "{}", show(&o));
        let s = String::from_utf8(o.stdout).unwrap();
        ids.insert(s.split(' ').next().unwrap().to_string());
    }
    assert_eq!(ids.len(), n);
    assert_eq!(entries(&e.new_dir("beta")).len(), n);
    assert_eq!(entries(&e.sent_dir("alpha")).len(), n);
    assert!(entries(&e.tmp_dir("beta")).is_empty());
}

#[test]
fn concurrent_receive_and_hook_do_not_duplicate() {
    let e = setup();
    let d = e.msg_dir("m", "x\n", &[]);
    for _ in 0..10 {
        e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);
    }
    let mut children = Vec::new();
    for i in 0..4 {
        let mut c = e.cmd(Some("beta"));
        if i % 2 == 0 {
            c.args(["receive", "shared"]);
        } else {
            c.env("LMM_HOOK_CMD", "printf '%s\\n' \"$LMM_RECEIVED\"")
                .args(["hook", "shared"]);
        }
        children.push(
            c.stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    let mut all = Vec::new();
    for c in children {
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success(), "{}", show(&o));
        all.extend(lines(&String::from_utf8(o.stdout).unwrap()));
    }
    let uniq: HashSet<_> = all.iter().collect();
    assert_eq!(all.len(), 10, "{all:?}");
    assert_eq!(uniq.len(), 10);
    assert_eq!(entries(&e.work.join("shared")).len(), 10);
    assert_eq!(entries(&e.cur_dir("beta")).len(), 10);
}

#[test]
fn post_fails_cleanly_when_box_removed_before_publish() {
    let e = setup();
    let d = e.msg_dir("m", "x\n", &[]);
    // Hold beta's mailbox lock exclusively so the post blocks before publishing.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(e.home.join("locks/box/beta.lock"))
        .unwrap();
    lock.lock().unwrap();
    let child = e
        .cmd(Some("alpha"))
        .args(["post", "-t", "beta", d.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    e.wait_for_workdir("beta");
    // While the post waits, its work directory is live (lock held).
    let tmp = e.tmp_dir("beta");
    assert_eq!(entries(&tmp).len(), 1);
    // Remove the mailbox the way `lmm box remove` would (we already hold
    // the box lock, so emulate steps 2-3 directly).
    let staged = e.home.join("boxes/.removing/beta");
    fs::create_dir_all(staged.parent().unwrap()).unwrap();
    fs::rename(e.home.join("boxes/beta"), &staged).unwrap();
    drop(lock);
    let o = child.wait_with_output().unwrap();
    assert_eq!(code(&o), 1, "{}", show(&o));
    assert!(!e.home.join("boxes/beta").exists(), "not recreated");
    assert!(o.stdout.is_empty());
    fs::remove_dir_all(staged).unwrap();
}

#[test]
fn box_remove_waits_for_publishing_and_refuses_live_sends() {
    let e = setup();
    let d = e.msg_dir("m", "x\n", &[]);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(e.home.join("locks/box/beta.lock"))
        .unwrap();
    lock.lock().unwrap();
    let child = e
        .cmd(Some("alpha"))
        .args(["post", "-t", "beta", d.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    e.wait_for_workdir("beta");
    drop(lock);
    // Race: either the post publishes first (remove refuses: new/ not empty)
    // or remove sees a live work directory and refuses.
    let o = e.run(None, &["box", "remove", "beta"]);
    let po = child.wait_with_output().unwrap();
    assert!(po.status.success(), "{}", show(&po));
    assert_eq!(code(&o), 1, "{}", show(&o));
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);
}

#[test]
fn stopped_sender_work_dir_survives_cleanup() {
    let e = setup();
    let d = e.msg_dir("m", "x\n", &[]);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(e.home.join("locks/box/beta.lock"))
        .unwrap();
    lock.lock().unwrap();
    let child = e
        .cmd(Some("alpha"))
        .args(["post", "-t", "beta", d.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    e.wait_for_workdir("beta");
    let pid = child.id() as i32;
    unsafe {
        libc::kill(pid, libc::SIGSTOP);
    }
    // Receive runs cleanup; the stopped sender's work dir must survive.
    e.ok(Some("beta"), &["receive"]);
    assert_eq!(entries(&e.tmp_dir("beta")).len(), 1);
    unsafe {
        libc::kill(pid, libc::SIGCONT);
    }
    drop(lock);
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);
    assert!(entries(&e.tmp_dir("beta")).is_empty());
}
