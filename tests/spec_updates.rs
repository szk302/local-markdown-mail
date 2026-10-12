//! Tests for the spec updates of 2026-10-11 (§5.4 tags are unit-tested in
//! `yaml.rs`) and non-UTF-8 paths (N3).

mod common;

use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::*;

fn setup() -> Env {
    let e = Env::new();
    e.create_box("alpha");
    e.create_box("beta");
    e
}

fn send(e: &Env) -> (String, PathBuf) {
    let d = e.msg_dir("m", "x\n", &[]);
    e.post("alpha", &["-t", "beta", d.to_str().unwrap()])
}

fn put_invalid(e: &Env) {
    let bad = e.new_dir("beta").join("0000_bad");
    fs::create_dir(&bad).unwrap();
    fs::write(bad.join("index.md"), "no front matter\n").unwrap();
}

/// §8.3: the built-in output exits 0 even if some messages failed.
#[test]
fn builtin_output_exits_0_with_partial_failure() {
    let e = setup();
    put_invalid(&e);
    let (_, p) = send(&e);
    let o = e.run(Some("beta"), &["hook"]);
    assert_eq!(code(&o), 0, "{}", show(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains(p.file_name().unwrap().to_str().unwrap()),
        "{out}"
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("invalid message"));
    assert_eq!(entries(&e.failed_dir("beta")).len(), 1);
    // Nothing extracted and a failure: still 1.
    put_invalid(&e);
    let o = e.run(Some("beta"), &["hook"]);
    assert_eq!(code(&o), 1, "{}", show(&o));
    assert!(o.stdout.is_empty());
}

fn pid_gone(pid: i32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return true;
    }
    // Reparented zombies (e.g. in containers without a reaping init).
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| {
            s.rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        })
        .unwrap_or(false)
}

fn read_pid(p: &Path) -> i32 {
    fs::read_to_string(p).unwrap().trim().parse().unwrap()
}

fn spawn_hook_with(e: &Env, cmd: &str) -> std::process::Child {
    e.cmd(Some("beta"))
        .env("LMM_HOOK_CMD", cmd)
        .arg("hook")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// §8.4: signals to `lmm hook` are forwarded to the command's process group.
#[test]
fn hook_forwards_signals_to_process_group() {
    for (sig, expected) in [
        (libc::SIGTERM, 143),
        (libc::SIGINT, 130),
        (libc::SIGHUP, 129),
    ] {
        let e = setup();
        send(&e);
        let pids = e.work.join("pids");
        let cmd = format!(
            "sleep 30 & echo $! > '{p}.tmp'; mv '{p}.tmp' '{p}'; wait",
            p = pids.display()
        );
        let child = spawn_hook_with(&e, &cmd);
        wait_until("grandchild pid", || pids.exists());
        let grandchild = read_pid(&pids);
        let start = Instant::now();
        // SAFETY: plain kill(2).
        unsafe { libc::kill(child.id() as i32, sig) };
        let o = child.wait_with_output().unwrap();
        assert_eq!(code(&o), expected, "{}", show(&o));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
        wait_until("grandchild to exit", || pid_gone(grandchild));
        // The message stays extracted (notification is at most once).
        assert_eq!(entries(&e.cur_dir("beta")).len(), 1);
    }
}

/// §8.4: a command ignoring the forwarded signal is SIGKILLed after 2 s.
#[test]
fn hook_kills_group_ignoring_forwarded_signal() {
    let e = setup();
    send(&e);
    let pids = e.work.join("pids");
    // Ignored dispositions are inherited by the background sleep.
    let cmd = format!(
        "trap '' TERM; sleep 30 & echo $! > '{p}.tmp'; mv '{p}.tmp' '{p}'; wait",
        p = pids.display()
    );
    let child = spawn_hook_with(&e, &cmd);
    wait_until("grandchild pid", || pids.exists());
    let grandchild = read_pid(&pids);
    let start = Instant::now();
    // SAFETY: plain kill(2).
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let o = child.wait_with_output().unwrap();
    let took = start.elapsed();
    assert_eq!(code(&o), 143, "{}", show(&o));
    assert!(took >= Duration::from_millis(1500), "{took:?}");
    assert!(took < Duration::from_secs(8), "{took:?}");
    wait_until("grandchild to exit", || pid_gone(grandchild));
}

/// §8.4: the wait after forwarding is bounded by the remaining timeout.
#[test]
fn hook_forward_wait_bounded_by_remaining_timeout() {
    let e = setup();
    fs::write(
        e.work.join(".lmm.yml"),
        "root: true\nhook:\n  timeout: 0.8\n",
    )
    .unwrap();
    send(&e);
    let ready = e.work.join("ready");
    let cmd = format!("trap '' TERM; touch '{}'; sleep 30", ready.display());
    let child = spawn_hook_with(&e, &cmd);
    wait_until("command start", || ready.exists());
    let start = Instant::now();
    // SAFETY: plain kill(2).
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let o = child.wait_with_output().unwrap();
    assert_eq!(code(&o), 143, "{}", show(&o));
    assert!(
        start.elapsed() < Duration::from_millis(1800),
        "{:?}",
        start.elapsed()
    );
}

/// §7.2, §6.7: dangling symlinks in the destination are usage errors.
#[test]
fn dangling_symlink_destinations_are_rejected() {
    let e = setup();
    send(&e);
    std::os::unix::fs::symlink(e.data_dir.join("boxes/newdir"), e.work.join("dang")).unwrap();
    for dir in ["dang", "dang/sub"] {
        let o = e.run(Some("beta"), &["receive", dir]);
        assert_eq!(code(&o), 2, "{dir}: {}", show(&o));
        assert!(String::from_utf8_lossy(&o.stderr).contains("dangling"));
        let o = e.run(Some("beta"), &["hook", dir]);
        assert_eq!(code(&o), 1, "{dir}: {}", show(&o));
        let o = e.run(Some("beta"), &["draft", dir]);
        assert_eq!(code(&o), 2, "{dir}: {}", show(&o));
    }
    assert!(!e.data_dir.join("boxes/newdir").exists());
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);
    // A symlink whose target exists still works.
    fs::create_dir(e.work.join("real")).unwrap();
    std::os::unix::fs::symlink(e.work.join("real"), e.work.join("ok")).unwrap();
    e.ok(Some("beta"), &["receive", "ok"]);
    assert_eq!(entries(&e.work.join("real")).len(), 1);
}

/// N3: non-UTF-8 paths are written byte for byte.
#[test]
fn non_utf8_paths_are_written_verbatim() {
    let e = setup();
    let name = std::ffi::OsString::from_vec(b"in-\xff".to_vec());
    let dest = e.work.join(&name);
    if fs::create_dir(&dest).is_err() {
        // File systems that require UTF-8 names (e.g. APFS).
        return;
    }
    fs::remove_dir(&dest).unwrap();
    send(&e);
    let o = e
        .cmd(Some("beta"))
        .arg("receive")
        .arg(&name)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", show(&o));
    let line = o.stdout.strip_suffix(b"\n").unwrap();
    let p = PathBuf::from(std::ffi::OsStr::from_bytes(line));
    assert!(p.join("index.md").exists(), "{p:?}");
    assert!(p.starts_with(&dest));

    // Hook: LMM_RECEIVED and LMM_RECEIVE_DIR are passed as raw bytes.
    send(&e);
    let outf = e.work.join("env.out");
    let o = e
        .cmd(Some("beta"))
        .env(
            "LMM_HOOK_CMD",
            format!(
                "printf '%s\\n%s' \"$LMM_RECEIVE_DIR\" \"$LMM_RECEIVED\" > '{}'",
                outf.display()
            ),
        )
        .arg("hook")
        .arg(&name)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", show(&o));
    let data = fs::read(&outf).unwrap();
    let (d, r) = data.split_at(data.iter().position(|&b| b == b'\n').unwrap());
    assert_eq!(d, dest.as_os_str().as_bytes());
    let r = PathBuf::from(std::ffi::OsStr::from_bytes(&r[1..]));
    assert!(r.join("index.md").exists(), "{r:?}");

    // post output and draft output.
    let draft = e
        .cmd(Some("alpha"))
        .arg("draft")
        .arg(e.work.join(&name).join("d"))
        .output()
        .unwrap();
    assert!(draft.status.success(), "{}", show(&draft));
    assert_eq!(
        draft.stdout.strip_suffix(b"\n").unwrap(),
        dest.join("d").as_os_str().as_bytes()
    );
}
