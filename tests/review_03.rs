//! Regression tests for review 03 (R03-1..R03-6).

mod common;

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::*;

fn setup() -> Env {
    let e = Env::new();
    e.create_box("alpha");
    e.create_box("beta");
    e
}

fn send(e: &Env) {
    let d = e.msg_dir("m", "x\n", &[]);
    e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);
}

/// R03-1: `missing/..` followed by a symlink into the data directory.
#[test]
fn dotdot_after_missing_component_cannot_bypass_check() {
    let e = setup();
    send(&e);
    let link = e.work.join("data-link");
    std::os::unix::fs::symlink(&e.home, &link).unwrap();
    let dest = format!(
        "{}/not-yet/../data-link/boxes/beta/inbox/new",
        e.work.display()
    );
    let o = e.run(Some("beta"), &["receive", &dest]);
    assert_eq!(code(&o), 2, "{}", show(&o));
    assert!(o.stdout.is_empty());
    assert!(!e.work.join("not-yet").exists(), "nothing created");
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);
    assert!(entries(&e.cur_dir("beta")).is_empty());
    // Same check in wait (2), hook (1) and draft (2).
    assert_eq!(
        code(&e.run(Some("beta"), &["wait", "--timeout", "0", &dest])),
        2
    );
    assert_eq!(code(&e.run(Some("beta"), &["hook", &dest])), 1);
    let draft = format!("{}/not-yet/../data-link/d", e.work.display());
    assert_eq!(code(&e.run(None, &["draft", &draft])), 2);
    assert!(!e.home.join("d").exists());
    // A symlink loop can't be judged: refused.
    std::os::unix::fs::symlink(e.work.join("loop"), e.work.join("loop")).unwrap();
    assert_eq!(code(&e.run(Some("beta"), &["receive", "loop/x"])), 2);
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);
}

/// R03-2: keys longer than YAML's implicit key limit survive post/receive.
#[test]
fn long_keys_round_trip() {
    let e = setup();
    for len in [1024, 1025, 3000] {
        let key = "a".repeat(len);
        let d = e.msg_dir(
            &format!("m{len}"),
            &format!("---\n? {key}\n: value\n---\nbody\n"),
            &[],
        );
        e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);
        let meta = format!("{}=v", "b".repeat(len));
        let d2 = e.msg_dir(&format!("n{len}"), "x\n", &[]);
        e.post("alpha", &["-t", "beta", "-m", &meta, d2.to_str().unwrap()]);
    }
    let out = e.ok(Some("beta"), &["receive"]);
    assert_eq!(lines(&out).len(), 6);
    assert!(entries(&e.failed_dir("beta")).is_empty());
    let p = &lines(&out)[0];
    let field = e.ok(None, &["show", p, "--field", &"a".repeat(1024)]);
    assert_eq!(field, "value\n");
}

/// R03-3: post refuses what the receiver would reject; aliases count
/// after expansion.
#[test]
fn post_rejects_messages_over_parser_limits() {
    let e = setup();
    // Fine as input, over the node limit after adding system keys.
    let items = vec!["0"; 99_990].join(",");
    let d = e.msg_dir("big", &format!("---\nitems: [{items}]\n---\n"), &[]);
    e.ok(None, &["show", "--json", d.to_str().unwrap()]);
    let o = e.run(Some("alpha"), &["post", "-t", "beta", d.to_str().unwrap()]);
    assert_eq!(code(&o), 1, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("could not be received"));
    // Deep once aliases are expanded: rejected as input already.
    let mut aliases = String::from("---\nk0: &a0 [value]\n");
    for i in 1..130 {
        aliases.push_str(&format!("k{i}: &a{i} [*a{}]\n", i - 1));
    }
    aliases.push_str("---\n");
    let d = e.msg_dir("deep", &aliases, &[]);
    assert_eq!(
        code(&e.run(None, &["show", d.to_str().unwrap(), "--json"])),
        1
    );
    assert_eq!(
        code(&e.run(Some("alpha"), &["post", "-t", "beta", d.to_str().unwrap()])),
        1
    );
    assert!(entries(&e.new_dir("beta")).is_empty());
    assert!(entries(&e.tmp_dir("beta")).is_empty());
}

/// R03-4: a signal while `wait` waits for the receive lock exits promptly.
#[test]
fn wait_interrupted_while_waiting_for_lock() {
    let e = setup();
    send(&e);
    let lockp = e.home.join("locks/receive/beta.lock");
    fs::create_dir_all(lockp.parent().unwrap()).unwrap();
    let holder = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lockp)
        .unwrap();
    holder.lock().unwrap();
    for (sig, expected) in [(libc::SIGTERM, 143), (libc::SIGINT, 130)] {
        let dir = e.work.join(format!("out-{sig}"));
        let child = e
            .cmd(Some("beta"))
            .args(["wait", "--timeout", "30"])
            .arg(&dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        e.wait_for_dir(&dir);
        // Wait until it has opened the contended lock file (Linux: /proc);
        // elsewhere fall back to a short delay.
        let fd_dir = std::path::PathBuf::from(format!("/proc/{}/fd", child.id()));
        if fd_dir.is_dir() {
            wait_until("receive lock to be opened", || {
                fs::read_dir(&fd_dir)
                    .map(|rd| {
                        rd.flatten()
                            .any(|f| fs::canonicalize(f.path()).is_ok_and(|t| t == lockp))
                    })
                    .unwrap_or(false)
            });
        } else {
            std::thread::sleep(Duration::from_millis(500));
        }
        let start = Instant::now();
        // SAFETY: plain kill(2).
        unsafe { libc::kill(child.id() as i32, sig) };
        let o = child.wait_with_output().unwrap();
        assert_eq!(code(&o), expected, "{}", show(&o));
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "{:?}",
            start.elapsed()
        );
        assert!(o.stdout.is_empty());
    }
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);
}

/// R03-5: non-UTF-8 environment values are used as given, never ignored.
#[test]
fn non_utf8_environment_values() {
    let e = Env::new();
    let raw = [e.tmp.path().as_os_str().as_bytes(), b"/data-\xff"].concat();
    let home = OsStr::from_bytes(&raw);
    if fs::create_dir(home).is_err() {
        return; // file system requires UTF-8 names
    }
    fs::remove_dir(home).unwrap();
    let o = e
        .cmd(None)
        .env("LMM_HOME", home)
        .args(["box", "create", "other"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 0, "{}", show(&o));
    assert!(std::path::Path::new(home).join("boxes/other").is_dir());
    assert!(!e.user_home.join(".lmm").exists(), "no fallback to ~/.lmm");
    // LMM_NAME must be text.
    let bad = OsStr::from_bytes(b"a\xff");
    let o = e
        .cmd(None)
        .env("LMM_NAME", bad)
        .args(["receive"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 2, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("not valid UTF-8"));
    let o = e
        .cmd(None)
        .env("LMM_NAME", bad)
        .arg("hook")
        .output()
        .unwrap();
    assert_eq!(code(&o), 1, "{}", show(&o));
    // LMM_CONFIG pointing to a missing non-UTF-8 path is an error, not "unset".
    let missing = OsStr::from_bytes(&[raw.as_slice(), b"/none-\xff.yml"].concat()).to_os_string();
    let o = e
        .cmd(None)
        .env("LMM_CONFIG", &missing)
        .args(["config", "show"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 2, "{}", show(&o));
}

/// F03-1: `{config_dir}` and `{cwd}` keep non-UTF-8 path bytes.
#[test]
fn placeholders_keep_non_utf8_paths() {
    let e = setup();
    let raw = [e.work.as_os_str().as_bytes(), b"/settings-\xff"].concat();
    let dir = std::path::PathBuf::from(OsStr::from_bytes(&raw));
    if fs::create_dir(&dir).is_err() {
        return; // file system requires UTF-8 names
    }
    fs::write(
        dir.join(".lmm.yml"),
        "receive:\n  dir: \"{config_dir}/inbox\"\ndraft:\n  dir: \"{cwd}/drafts\"\n",
    )
    .unwrap();
    send(&e);
    let o = e
        .cmd(Some("beta"))
        .current_dir(&dir)
        .arg("receive")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(entries(&dir.join("inbox")).len(), 1);
    let o = e
        .cmd(Some("beta"))
        .current_dir(&dir)
        .arg("draft")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", show(&o));
    assert!(
        o.stdout
            .starts_with(dir.join("drafts").as_os_str().as_bytes())
    );
    assert_eq!(entries(&dir.join("drafts")).len(), 1);
    // A text value can't hold the bytes: explicit config error.
    fs::write(
        dir.join(".lmm.local.yml"),
        "post:\n  meta:\n    where: \"{config_dir}\"\n",
    )
    .unwrap();
    let m = e.msg_dir("m2", "x\n", &[]);
    let o = e
        .cmd(Some("alpha"))
        .current_dir(&dir)
        .args(["post", "-t", "beta", m.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(code(&o), 2, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("not valid UTF-8"));
}

/// F03-2: `config show` output stays valid YAML with long keys.
#[test]
fn config_show_long_keys_are_valid_yaml() {
    let e = Env::new();
    let key = "a".repeat(1025);
    fs::write(
        e.work.join(".lmm.yml"),
        format!("root: true\npost:\n  meta:\n    ? {key}\n    : value\n    short: v\n"),
    )
    .unwrap();
    for origin in [false, true] {
        let mut args = vec!["config", "show"];
        if origin {
            args.push("--origin");
        }
        let out = e.ok(None, &args);
        let map = lmm::yaml::parse_map(&out).unwrap_or_else(|err| panic!("{origin}: {err}\n{out}"));
        let Some(lmm::yaml::Node::Map(post)) = map.get("post") else {
            panic!("{out}")
        };
        let Some(lmm::yaml::Node::Map(meta)) = post.get("meta") else {
            panic!("{out}")
        };
        assert_eq!(meta.get_str(&key), Some("value"));
        assert_eq!(meta.get_str("short"), Some("v"));
    }
}

/// R03-6: removing again finishes an interrupted removal.
#[test]
fn box_remove_finishes_interrupted_removal() {
    let e = setup();
    send(&e);
    let staged = e.home.join("boxes/.removing/alpha");
    fs::rename(e.home.join("boxes/alpha"), &staged).unwrap();
    e.ok(None, &["box", "remove", "alpha"]);
    assert!(!staged.exists());
    // Nothing left: the usual "does not exist" error.
    let o = e.run(None, &["box", "remove", "alpha"]);
    assert_eq!(code(&o), 1, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("does not exist"));
    // A never-created address doesn't get a lock file.
    assert_eq!(code(&e.run(None, &["box", "remove", "gamma"])), 1);
    assert!(!e.home.join("locks/receive/gamma.lock").exists());
}
