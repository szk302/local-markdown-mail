mod common;

use std::fs;

use common::*;

#[test]
fn config_layering_and_origin() {
    let e = Env::new();
    let global = e.home.join("config.yml");
    fs::create_dir_all(&e.home).unwrap();
    fs::write(
        &global,
        "hook:\n  timeout: 5\n  max_messages: 3\npost:\n  meta:\n    a: g\n    b: g\nmax_message_size: 10M\n",
    )
    .unwrap();
    // parent (work) has root: true; add files at work and work/sub.
    fs::write(
        e.work.join(".lmm.yml"),
        "root: true\npost:\n  meta:\n    b: shared\n",
    )
    .unwrap();
    let sub = e.work.join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(
        sub.join(".lmm.yml"),
        "receive:\n  dir: shared-dir\nhook:\n  timeout: 7\n",
    )
    .unwrap();
    fs::write(
        sub.join(".lmm.local.yml"),
        "receive:\n  dir: local-dir\nmax_message_size: 1K\n",
    )
    .unwrap();
    // Above root: must be ignored.
    fs::write(
        e.tmp.path().join(".lmm.yml"),
        "hook:\n  cmd: should-not-apply\n",
    )
    .unwrap();

    let o = e
        .cmd(None)
        .args(["config", "show", "-C", "sub", "--origin"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", show(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    let err = String::from_utf8_lossy(&o.stderr);
    let local = sub.join(".lmm.local.yml").display().to_string();
    let shared = sub.join(".lmm.yml").display().to_string();
    let top = e.work.join(".lmm.yml").display().to_string();
    let g = global.display().to_string();
    assert!(
        out.contains(&format!("#   {local}\n#   {shared}\n#   {top}\n#   {g}\n")),
        "{out}"
    );
    assert!(
        out.contains(&format!("  dir: local-dir  # {local}\n")),
        "{out}"
    );
    assert!(
        out.contains(&format!("  timeout: 7  # {shared}\n")),
        "{out}"
    );
    assert!(
        out.contains(&format!("  max_messages: 3  # {g}\n")),
        "{out}"
    );
    assert!(out.contains(&format!("    a: g  # {g}\n")), "{out}");
    assert!(out.contains(&format!("    b: shared  # {top}\n")), "{out}");
    assert!(
        out.contains(&format!("max_message_size: \"10M\"  # {g}\n")),
        "{out}"
    );
    assert!(
        out.contains("  dir: \".lmm-drafts/{name}\"  # (default)\n"),
        "{out}"
    );
    assert!(!out.contains("should-not-apply"), "{out}");
    assert!(
        err.contains("max_message_size is only valid in the global config"),
        "{err}"
    );

    // Environment overrides.
    let o = e
        .cmd(None)
        .env("LMM_RECEIVE_DIR", "/env/dir")
        .env("LMM_HOOK_CMD", "echo hi")
        .args(["config", "show", "-C", "sub", "--origin"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("  dir: /env/dir  # LMM_RECEIVE_DIR\n"),
        "{out}"
    );
    assert!(out.contains("  cmd: echo hi  # LMM_HOOK_CMD\n"), "{out}");
}

#[test]
fn config_errors() {
    let e = Env::new();
    let cases = [
        "hook:\n  timeout: 0\n",
        "hook:\n  max_messages: 31\n",
        "hook:\n  max_messages: 0\n",
        "post:\n  reply_to: Bad\n",
        "post:\n  meta:\n    lmm_x: a\n",
        "post:\n  meta:\n    n: 1\n",
        "receive:\n  dir: null\n",
        "receive:\n  dir: \"{unknown}\"\n",
        "receive:\n  dir: [a]\n",
        "a: 1\na: 2\n",
        "- list\n",
        "root: yes\n",
    ];
    for c in cases {
        fs::write(e.work.join(".lmm.local.yml"), c).unwrap();
        let o = e.run(None, &["config", "show"]);
        assert_eq!(code(&o), 2, "{c}: {}", show(&o));
    }
    // Unknown keys warn.
    fs::write(e.work.join(".lmm.local.yml"), "nope: 1\nhook:\n  zzz: 1\n").unwrap();
    let o = e.run(None, &["config", "show"]);
    assert_eq!(code(&o), 0);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("nope") && err.contains("hook.zzz"), "{err}");
    fs::remove_file(e.work.join(".lmm.local.yml")).unwrap();
    // LMM_CONFIG must exist.
    let o = e
        .cmd(None)
        .env("LMM_CONFIG", e.work.join("missing.yml"))
        .args(["config", "show"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 2);
}

#[test]
fn unreadable_config_is_skipped() {
    let e = Env::new();
    let p = e.work.join(".lmm.local.yml");
    fs::write(&p, "hook:\n  timeout: 9\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&p, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&p).is_ok() {
        // Running as root: permissions are not enforced.
        return;
    }
    let o = e.run(None, &["config", "show"]);
    assert_eq!(code(&o), 0, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("cannot read config"));
    assert!(String::from_utf8_lossy(&o.stdout).contains("timeout: 30"));
}
