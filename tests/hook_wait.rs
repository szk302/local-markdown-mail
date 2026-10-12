mod common;

use std::fs;
use std::io::Write;
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::*;

fn setup() -> Env {
    let e = Env::new();
    e.create_box("alpha");
    e.create_box("beta");
    e
}

fn send(e: &Env, rel: &str, index: &str) -> (String, std::path::PathBuf) {
    let d = e.msg_dir(rel, index, &[]);
    e.post("alpha", &["-t", "beta", d.to_str().unwrap()])
}

#[test]
fn hook_participation_rules() {
    let e = setup();
    // Not participating.
    let o = e.run(None, &["hook"]);
    assert_eq!(code(&o), 0);
    assert!(o.stdout.is_empty() && o.stderr.is_empty());
    // Missing mailbox: warning, exit 0.
    let o = e.run(Some("gone"), &["hook"]);
    assert_eq!(code(&o), 0);
    assert!(o.stdout.is_empty());
    assert!(!o.stderr.is_empty());
    // Invalid LMM_NAME: 1, not 2.
    assert_eq!(code(&e.run(Some("Bad"), &["hook"])), 1);
    // No new mail: silent.
    let o = e.run(Some("beta"), &["hook"]);
    assert_eq!(code(&o), 0);
    assert!(o.stdout.is_empty() && o.stderr.is_empty());
    assert!(!e.work.join(".lmm-inbox").exists());
}

#[test]
fn hook_builtin_output_and_max_messages() {
    let e = setup();
    fs::write(
        e.work.join(".lmm.yml"),
        "root: true\nhook:\n  max_messages: 2\n",
    )
    .unwrap();
    for i in 0..3 {
        send(&e, &format!("m{i}"), "x\n");
    }
    let o = e.run(Some("beta"), &["hook"]);
    assert_eq!(code(&o), 0, "{}", show(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    let dest = e.work.join(".lmm-inbox/beta").display().to_string();
    assert_eq!(out.matches(&dest).count(), 2, "{out}");
    assert!(out.contains("1 more message"), "{out}");
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);
    // Next run gets the rest; the one after is silent.
    let o = e.run(Some("beta"), &["hook"]);
    assert_eq!(String::from_utf8_lossy(&o.stdout).matches(&dest).count(), 1);
    let o = e.run(Some("beta"), &["hook"]);
    assert!(o.stdout.is_empty());
}

#[test]
fn hook_command_env_stdin_and_exit_code() {
    let e = setup();
    let (_, p) = send(&e, "m", "x\n");
    let name = p.file_name().unwrap().to_str().unwrap().to_string();
    let cmd = r#"read -r line; printf '{"in":"%s","n":"%s","c":"%s","p":"%s","d":"%s"}\n' "$line" "$LMM_NAME" "$LMM_RECEIVED_COUNT" "$LMM_PENDING_COUNT" "$LMM_RECEIVE_DIR"; printf '%s' "$LMM_RECEIVED" >&2; exit 7"#;
    let mut child = e
        .cmd(Some("beta"))
        .env("LMM_HOOK_CMD", cmd)
        .arg("hook")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"from-cli\n")
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(code(&o), 7, "{}", show(&o));
    let dest = e.work.join(".lmm-inbox/beta");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("stdout is pure JSON");
    assert_eq!(v["in"], "from-cli");
    assert_eq!(v["n"], "beta");
    assert_eq!(v["c"], "1");
    assert_eq!(v["p"], "0");
    assert_eq!(v["d"], dest.display().to_string());
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        dest.join(&name).display().to_string()
    );
}

#[test]
fn hook_jq_example_from_spec() {
    let e = setup();
    if std::process::Command::new("jq")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("jq not available; skipping");
        return;
    }
    fs::write(
        e.work.join(".lmm.yml"),
        "root: true\nreceive:\n  dir: \"{config_dir}/.lmm-inbox/{name}\"\nhook:\n  cmd: >-\n    jq -n --arg paths \"$LMM_RECEIVED\"\n    '{decision: \"block\", reason: (\"New messages:\\n\" + $paths)}'\n",
    )
    .unwrap();
    send(&e, "m", "x\n");
    let o = e.run(Some("beta"), &["hook"]);
    assert_eq!(code(&o), 0, "{}", show(&o));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["decision"], "block");
    assert!(v["reason"].as_str().unwrap().contains(".lmm-inbox/beta/"));
}

#[test]
fn hook_timeout_kills_process_group() {
    let e = setup();
    fs::write(
        e.work.join(".lmm.yml"),
        "root: true\nhook:\n  timeout: 0.5\n",
    )
    .unwrap();
    send(&e, "m", "x\n");
    // A grandchild keeps stdout open; the whole group must be killed.
    let start = Instant::now();
    let o = e
        .cmd(Some("beta"))
        .env("LMM_HOOK_CMD", "sleep 30 & sleep 30; wait")
        .arg("hook")
        .output()
        .unwrap();
    let took = start.elapsed();
    assert_eq!(code(&o), 1, "{}", show(&o));
    assert!(took < Duration::from_secs(10), "took {took:?}");
    assert!(String::from_utf8_lossy(&o.stderr).contains("timed out"));
    // The message stays extracted.
    assert_eq!(entries(&e.cur_dir("beta")).len(), 1);
}

#[test]
fn hook_config_error_is_1() {
    let e = setup();
    send(&e, "m", "x\n");
    fs::write(
        e.work.join(".lmm.yml"),
        "root: true\nhook:\n  max_messages: 99\n",
    )
    .unwrap();
    let o = e.run(Some("beta"), &["hook"]);
    assert_eq!(code(&o), 1, "{}", show(&o));
    let o = e.run(
        Some("beta"),
        &["hook", e.data_dir.join("x").to_str().unwrap()],
    );
    assert_eq!(code(&o), 1, "{}", show(&o));
}

#[test]
fn wait_filters_and_leaves_others() {
    let e = setup();
    e.create_box("gamma");
    let (id, p) = send(&e, "req", "---\nlmm_thread_id: t1\nkind: report\n---\n");
    // Unrelated message from gamma.
    let d = e.msg_dir("other", "x\n", &[]);
    e.post("gamma", &["-t", "beta", d.to_str().unwrap()]);
    let out = e.ok(
        Some("beta"),
        &[
            "wait",
            "--from",
            "alpha",
            "--thread",
            "t1",
            "--meta",
            "kind=report",
            "--timeout",
            "5",
        ],
    );
    assert_eq!(lines(&out).len(), 1);
    assert!(out.contains(p.file_name().unwrap().to_str().unwrap()));
    assert_eq!(
        entries(&e.new_dir("beta")).len(),
        1,
        "gamma's message stays"
    );
    let extracted = out.trim_end().to_string();
    // in-reply-to
    let r = e.msg_dir("reply", "ok\n", &[]);
    e.post("beta", &["--reply", &extracted, r.to_str().unwrap()]);
    let out = e.ok(
        Some("alpha"),
        &["wait", "--in-reply-to", &id, "--timeout", "5"],
    );
    assert_eq!(lines(&out).len(), 1);
    // Numeric meta values never match.
    let n = e.msg_dir("num", "---\nversion: 1.10\n---\n", &[]);
    e.post("alpha", &["-t", "beta", n.to_str().unwrap()]);
    let o = e.run(
        Some("beta"),
        &["wait", "--meta", "version=1.10", "--timeout", "1.2"],
    );
    assert_eq!(code(&o), 3, "{}", show(&o));
    // Without conditions: one message per run (oldest first) by default,
    // everything with --all.
    let out = e.ok(Some("beta"), &["wait", "--timeout", "5"]);
    assert_eq!(lines(&out).len(), 1);
    assert_eq!(entries(&e.new_dir("beta")).len(), 1);
    assert_eq!(
        code(&e.run(Some("beta"), &["wait", "--all", "--max", "2"])),
        2
    );
    let n2 = e.msg_dir("num2", "x\n", &[]);
    e.post("alpha", &["-t", "beta", n2.to_str().unwrap()]);
    let out = e.ok(Some("beta"), &["wait", "--all", "--timeout", "5"]);
    assert_eq!(lines(&out).len(), 2);
    assert!(entries(&e.new_dir("beta")).is_empty());
}

#[test]
fn wait_moves_invalid_to_failed_and_keeps_waiting() {
    let e = setup();
    let bad = e.new_dir("beta").join("0000_bad");
    fs::create_dir(&bad).unwrap();
    fs::write(bad.join("index.md"), "nope").unwrap();
    let o = e.run(Some("beta"), &["wait", "--timeout", "1"]);
    assert_eq!(code(&o), 3, "{}", show(&o));
    assert_eq!(entries(&e.failed_dir("beta")).len(), 1);
}

#[test]
fn wait_timeout_usage_and_missing_box() {
    let e = setup();
    let start = Instant::now();
    let o = e.run(Some("beta"), &["wait", "--timeout", "0.3"]);
    assert_eq!(code(&o), 3);
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(code(&e.run(Some("beta"), &["wait", "--from", "Bad"])), 2);
    assert_eq!(code(&e.run(None, &["wait"])), 2);
    assert_eq!(code(&e.run(Some("gone"), &["wait"])), 1);
    // Mailbox removed while waiting.
    let dir = e.work.join("removed-while-waiting");
    let child = e
        .cmd(Some("beta"))
        .args(["wait", "--timeout", "20"])
        .arg(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    e.wait_for_dir(&dir);
    e.ok(None, &["box", "remove", "beta"]);
    let o = child.wait_with_output().unwrap();
    assert_eq!(code(&o), 1, "{}", show(&o));
}

#[test]
fn wait_delivers_when_message_arrives_and_handles_signals() {
    let e = setup();
    let child = e
        .cmd(Some("beta"))
        .args(["wait", "--timeout", "20"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    e.wait_for_dir(&e.work.join(".lmm-inbox/beta"));
    let (_, p) = send(&e, "late", "x\n");
    let o = child.wait_with_output().unwrap();
    assert_eq!(code(&o), 0, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stdout).contains(p.file_name().unwrap().to_str().unwrap()));

    for (sig, expected) in [(libc::SIGTERM, 143), (libc::SIGINT, 130)] {
        // A fresh receive directory per run tells us when the handlers are
        // installed (they are set up before the directory is created).
        let dir = e.work.join(format!("sig-{sig}"));
        let child = e
            .cmd(Some("beta"))
            .arg("wait")
            .arg(&dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        e.wait_for_dir(&dir);
        unsafe {
            libc::kill(child.id() as i32, sig);
        }
        let o = child.wait_with_output().unwrap();
        assert_eq!(code(&o), expected, "{}", show(&o));
        assert!(o.stdout.is_empty());
    }
}
