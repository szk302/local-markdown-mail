//! Regression tests for review findings (2026-10-11 review).

mod common;

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::process::Stdio;

use common::*;

fn setup() -> Env {
    let e = Env::new();
    e.create_box("alpha");
    e.create_box("beta");
    e
}

fn send(e: &Env) -> (String, std::path::PathBuf) {
    let d = e.msg_dir("m", "x\n", &[]);
    e.post("alpha", &["-t", "beta", d.to_str().unwrap()])
}

/// M1: `config show -C ..` walks the real ancestors and lists each file once.
#[test]
fn config_show_c_with_dotdot() {
    let e = Env::new();
    // work/.lmm.yml has root: true; add a child and a grandchild.
    let sub = e.work.join("sub");
    let subsub = sub.join("subsub");
    fs::create_dir_all(&subsub).unwrap();
    fs::write(sub.join(".lmm.yml"), "hook:\n  timeout: 3\n").unwrap();
    fs::write(subsub.join(".lmm.yml"), "hook:\n  timeout: 9\n").unwrap();
    let o = e
        .cmd(None)
        .current_dir(&subsub)
        .args(["config", "show", "-C", "..", "--origin"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", show(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    let expected = format!(
        "#   {}\n#   {}\n",
        sub.join(".lmm.yml").display(),
        e.work.join(".lmm.yml").display()
    );
    assert!(out.contains(&expected), "{out}");
    assert!(
        !out.contains("subsub"),
        "child config must not be read: {out}"
    );
    assert!(!out.contains(".."), "{out}");
    assert!(out.contains("timeout: 3  #"), "{out}");
}

/// L1: a huge hook.timeout is a config error (no panic; hook returns 1
/// before receiving anything).
#[test]
fn hook_huge_timeout_is_config_error() {
    let e = setup();
    send(&e);
    for v in ["1e20", "1e300", "123456789012345678901234567890"] {
        fs::write(
            e.work.join(".lmm.yml"),
            format!("root: true\nhook:\n  timeout: {v}\n  cmd: \"true\"\n"),
        )
        .unwrap();
        let o = e.run(Some("beta"), &["hook"]);
        assert_eq!(code(&o), 1, "{v}: {}", show(&o));
        assert!(!String::from_utf8_lossy(&o.stderr).contains("panicked"));
        assert_eq!(code(&e.run(None, &["config", "show"])), 2, "{v}");
    }
    // Nothing was received, so the message is still waiting.
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);
    // A large but representable timeout works (no Instant overflow).
    fs::write(
        e.work.join(".lmm.yml"),
        "root: true\nhook:\n  timeout: 1e15\n  cmd: \"echo ok\"\n",
    )
    .unwrap();
    let o = e.run(Some("beta"), &["hook"]);
    assert_eq!(code(&o), 0, "{}", show(&o));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "ok\n");
}

/// L2: a huge `wait --timeout` is a usage error.
#[test]
fn wait_huge_timeout_is_usage_error() {
    let e = setup();
    let o = e.run(Some("beta"), &["wait", "--timeout", "1e30"]);
    assert_eq!(code(&o), 2, "{}", show(&o));
    assert!(!String::from_utf8_lossy(&o.stderr).contains("panicked"));
    // Large but representable: returns immediately when a message exists.
    send(&e);
    let o = e.run(Some("beta"), &["wait", "--timeout", "1e15"]);
    assert_eq!(code(&o), 0, "{}", show(&o));
}

/// L3: without LMM_NAME, `lmm hook` exits 0 even if the current directory
/// or HOME is unavailable.
#[test]
fn hook_without_name_ignores_missing_cwd_and_home() {
    let e = Env::new();
    let gone = e.work.join("gone");
    fs::create_dir(&gone).unwrap();
    let mut c = e.cmd(None);
    c.arg("hook").current_dir(&gone);
    let g = gone.clone();
    // SAFETY: only async-signal-safe work (rmdir) between fork and exec.
    unsafe {
        c.pre_exec(move || {
            let p = std::ffi::CString::new(g.as_os_str().as_bytes()).unwrap();
            libc::rmdir(p.as_ptr());
            Ok(())
        });
    }
    let o = c.output().unwrap();
    assert!(!gone.exists());
    assert_eq!(code(&o), 0, "{}", show(&o));
    assert!(o.stdout.is_empty() && o.stderr.is_empty(), "{}", show(&o));

    let o = e
        .cmd(None)
        .env_remove("HOME")
        .env_remove("LMM_HOME")
        .arg("hook")
        .output()
        .unwrap();
    assert_eq!(code(&o), 0, "{}", show(&o));
}

/// L4: hidden entries in new/ and failed/ are not messages.
#[test]
fn hidden_entries_are_not_counted() {
    let e = setup();
    fs::write(e.new_dir("beta").join(".DS_Store"), "").unwrap();
    fs::write(e.failed_dir("beta").join(".DS_Store"), "").unwrap();
    assert_eq!(
        e.ok(None, &["box", "list"]),
        "alpha\tnew=0\tfailed=0\nbeta\tnew=0\tfailed=0\n"
    );
    // hook takes the cheap path: no config read, no receive directory.
    fs::write(
        e.work.join(".lmm.yml"),
        "root: true\nhook:\n  max_messages: 99\n",
    )
    .unwrap();
    let o = e.run(Some("beta"), &["hook"]);
    assert_eq!(code(&o), 0, "{}", show(&o));
    assert!(o.stdout.is_empty() && o.stderr.is_empty(), "{}", show(&o));
    assert!(!e.work.join(".lmm-inbox").exists());
    fs::write(e.work.join(".lmm.yml"), "root: true\n").unwrap();
    // box remove does not need --force.
    e.ok(None, &["box", "remove", "beta"]);
    assert!(!e.home.join("boxes/beta").exists());
}

/// L5: a hand-placed message with a long lmm_thread_id is received.
#[test]
fn long_thread_id_is_received() {
    let e = setup();
    let d = e.new_dir("beta").join("hand");
    fs::create_dir(&d).unwrap();
    let extra = format!("lmm_thread_id: {}\n", "x".repeat(129));
    fs::write(
        d.join("index.md"),
        received_index(
            "a1b2c3d4-e5f6-4890-abcd-ef1234567890",
            "alpha",
            "beta",
            &extra,
        ),
    )
    .unwrap();
    let out = e.ok(Some("beta"), &["receive"]);
    assert_eq!(lines(&out).len(), 1);
    assert!(entries(&e.failed_dir("beta")).is_empty());
}

/// L7: large integers in custom keys survive post and show --json.
#[test]
fn big_integer_round_trip() {
    let e = setup();
    let big = "123456789012345678901234567890123456789012";
    let d = e.msg_dir("m", &format!("---\nbig: {big}\n---\n"), &[]);
    let (_, p) = e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);
    let idx = fs::read_to_string(p.join("index.md")).unwrap();
    assert!(idx.contains(&format!("big: {big}\n")), "{idx}");
    let out = e.ok(None, &["show", p.to_str().unwrap(), "--json"]);
    assert!(out.contains(&format!("\"big\":{big}")), "{out}");
    let out = e.ok(None, &["show", p.to_str().unwrap(), "--field", "big"]);
    assert_eq!(out, format!("{big}\n"));
}

/// L8: PATH must be a directory or a file named index.md.
#[test]
fn show_rejects_other_files() {
    let e = setup();
    let d = e.msg_dir("m", "x\n", &[("screen.png", b"PNG")]);
    let o = e.run(
        None,
        &["show", d.join("screen.png").to_str().unwrap(), "--json"],
    );
    assert_eq!(code(&o), 2, "{}", show(&o));
    assert!(o.stdout.is_empty());
    // A missing path is a runtime error.
    assert_eq!(
        code(&e.run(None, &["show", d.join("nope").to_str().unwrap()])),
        1
    );
    // index.md and the directory still work.
    e.ok(None, &["show", d.join("index.md").to_str().unwrap()]);
    e.ok(None, &["show", d.to_str().unwrap()]);
}

/// N1: an invalid max_message_size in a directory config is ignored.
#[test]
fn invalid_max_message_size_in_directory_config_is_ignored() {
    let e = Env::new();
    fs::write(
        e.work.join(".lmm.yml"),
        "root: true\nmax_message_size: lots\n",
    )
    .unwrap();
    let o = e.run(None, &["config", "show"]);
    assert_eq!(code(&o), 0, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("only valid in the global config"));
    // Still an error in the global config.
    let g = e.tmp.path().join("g.yml");
    fs::write(&g, "max_message_size: lots\n").unwrap();
    let o = e
        .cmd(None)
        .env("LMM_CONFIG", &g)
        .args(["config", "show"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 2, "{}", show(&o));
}

/// Spawn with stdout connected to a pipe whose read end is already closed.
fn run_with_closed_stdout(e: &Env, name: Option<&str>, args: &[&str]) -> std::process::Output {
    let mut fds = [0i32; 2];
    // SAFETY: plain pipe(2).
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: closing the read end we own.
    unsafe { libc::close(fds[0]) };
    // SAFETY: we own fds[1]; Stdio takes ownership.
    let stdout = unsafe { <Stdio as std::os::fd::FromRawFd>::from_raw_fd(fds[1]) };
    e.cmd(name)
        .args(args)
        .stdout(stdout)
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

/// L6: an unwritable receive directory stops after the first message
/// instead of logging the same error for every message.
#[test]
fn unwritable_destination_stops_processing() {
    use std::os::unix::fs::PermissionsExt;
    let e = setup();
    let d = e.msg_dir("m", "x\n", &[]);
    for _ in 0..3 {
        e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);
    }
    let dest = e.work.join("ro");
    fs::create_dir(&dest).unwrap();
    fs::set_permissions(&dest, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::create_dir(dest.join("probe")).is_ok() {
        // Running as root: permissions are not enforced.
        return;
    }
    let o = e.run(Some("beta"), &["receive", "ro"]);
    fs::set_permissions(&dest, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code(&o), 1, "{}", show(&o));
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(
        err.matches("lmm: error: cannot extract").count(),
        1,
        "{err}"
    );
    assert!(err.contains("stopping"), "{err}");
    assert!(o.stdout.is_empty());
    assert_eq!(entries(&e.new_dir("beta")).len(), 3);
    assert!(entries(&e.failed_dir("beta")).is_empty());
}

/// N2: a closed stdout doesn't panic; post still succeeds (delivery is
/// committed), draft reports an error.
#[test]
fn closed_stdout_does_not_panic() {
    let e = setup();
    let d = e.msg_dir("m", "x\n", &[]);
    let o = run_with_closed_stdout(
        &e,
        Some("alpha"),
        &["post", "-t", "beta", d.to_str().unwrap()],
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(code(&o), 0, "{err}");
    assert!(!err.contains("panicked"), "{err}");
    assert!(err.contains("delivered"), "{err}");
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);

    let o = run_with_closed_stdout(&e, Some("alpha"), &["draft"]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(code(&o), 1, "{err}");
    assert!(!err.contains("panicked"), "{err}");

    let o = run_with_closed_stdout(&e, None, &["config", "show"]);
    assert_eq!(code(&o), 1);
    assert!(!String::from_utf8_lossy(&o.stderr).contains("panicked"));
}

/// N4: non-UTF-8 argv doesn't panic.
#[test]
fn non_utf8_argv_does_not_panic() {
    let e = Env::new();
    let bad = std::ffi::OsStr::from_bytes(b"\xff");
    let o = e.cmd(None).arg0(bad).arg("--nope").output().unwrap();
    assert_eq!(code(&o), 2, "{}", show(&o));
    assert!(!String::from_utf8_lossy(&o.stderr).contains("panicked"));
    let o = e.cmd(None).arg(bad).output().unwrap();
    assert_eq!(code(&o), 2, "{}", show(&o));
    // `hook` with a usage error and a non-UTF-8 argv[0]: 1, never 2.
    let o = e
        .cmd(None)
        .arg0(bad)
        .args(["hook", "--nope"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 1, "{}", show(&o));
    // A non-UTF-8 DIR is a valid path (no LMM_NAME: nothing to do).
    let o = e.cmd(None).arg("hook").arg(bad).output().unwrap();
    assert_eq!(code(&o), 0, "{}", show(&o));
}
