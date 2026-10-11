mod common;

use std::fs;

use common::*;

fn setup() -> Env {
    let e = Env::new();
    e.create_box("alpha");
    e.create_box("beta");
    e
}

#[test]
fn show_formats() {
    let e = setup();
    let d = e.msg_dir(
        "m",
        "---\nlmm_subject: Hello\nversion: 1.10\nflag: true\nlist: [a, 1]\nwhen: 2026-10-05\n---\nBody\n",
        &[("b.txt", b"b"), ("a.txt", b"a")],
    );
    let (id, p) = e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);
    let raw = e.ok(None, &["show", p.to_str().unwrap()]);
    assert_eq!(raw, fs::read_to_string(p.join("index.md")).unwrap());
    let via_index = e.ok(None, &["show", p.join("index.md").to_str().unwrap()]);
    assert_eq!(via_index, raw);

    let j: serde_json::Value =
        serde_json::from_str(&e.ok(None, &["show", "--json", p.to_str().unwrap()])).unwrap();
    assert_eq!(j["path"], p.display().to_string());
    assert_eq!(j["body"], "Body\n");
    assert_eq!(j["attachments"], serde_json::json!(["a.txt", "b.txt"]));
    assert_eq!(j["frontmatter"]["lmm_id"], id.as_str());
    assert_eq!(j["frontmatter"]["version"], 1.1);
    assert_eq!(j["frontmatter"]["when"], "2026-10-05");
    let keys: Vec<_> = j["frontmatter"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        keys,
        [
            "lmm_id",
            "lmm_from",
            "lmm_to",
            "lmm_created_at",
            "lmm_subject",
            "version",
            "flag",
            "list",
            "when"
        ]
    );

    let ps = p.to_str().unwrap();
    assert_eq!(
        e.ok(None, &["show", ps, "--field", "lmm_subject"]),
        "Hello\n"
    );
    assert_eq!(e.ok(None, &["show", ps, "--field", "flag"]), "true\n");
    assert_eq!(e.ok(None, &["show", ps, "--field", "list"]), "[\"a\",1]\n");
    let o = e.run(None, &["show", ps, "--field", "missing"]);
    assert_eq!(code(&o), 1);
    assert!(o.stdout.is_empty());
    // Usage errors.
    assert_eq!(code(&e.run(None, &["show"])), 2);
    assert_eq!(code(&e.run(Some("beta"), &["show", ps, "--id", &id])), 2);
    assert_eq!(
        code(&e.run(None, &["show", ps, "--json", "--field", "x"])),
        2
    );
    // Missing index.
    assert_eq!(code(&e.run(None, &["show", e.work.to_str().unwrap()])), 1);
}

#[test]
fn show_by_id() {
    let e = setup();
    let d = e.msg_dir("m", "x\n", &[]);
    let (id, p) = e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);
    // Broken candidate is skipped with a warning.
    let broken = e.new_dir("beta").join("0000-broken");
    fs::create_dir(&broken).unwrap();
    let o = e.run(Some("beta"), &["show", "--id", &id, "--field", "lmm_id"]);
    assert_eq!(code(&o), 0, "{}", show(&o));
    assert_eq!(String::from_utf8_lossy(&o.stdout), format!("{id}\n"));
    assert!(String::from_utf8_lossy(&o.stderr).contains("broken"));
    // Showing does not change state.
    assert!(p.exists());
    // Sender finds it in sent/.
    let j: serde_json::Value =
        serde_json::from_str(&e.ok(Some("alpha"), &["show", "--id", &id, "--json"])).unwrap();
    assert!(j["path"].as_str().unwrap().contains("/alpha/sent/"));
    // Self-addressed: inbox wins over sent.
    let (sid, _) = e.post("alpha", &["-t", "alpha", d.to_str().unwrap()]);
    let j: serde_json::Value =
        serde_json::from_str(&e.ok(Some("alpha"), &["show", "--id", &sid, "--json"])).unwrap();
    assert!(j["path"].as_str().unwrap().contains("/alpha/inbox/new/"));
    // Not found / errors.
    assert_eq!(code(&e.run(Some("beta"), &["show", "--id", &sid])), 1);
    assert_eq!(code(&e.run(None, &["show", "--id", &sid])), 2);
    assert_eq!(code(&e.run(Some("nobody"), &["show", "--id", &sid])), 1);
}

#[test]
fn draft_then_post() {
    let e = setup();
    let out = e.ok(
        Some("alpha"),
        &[
            "draft",
            "-s",
            "Review result",
            "--thread",
            "t-1",
            "-m",
            "kind=review",
        ],
    );
    let d = std::path::PathBuf::from(out.trim_end());
    assert!(d.starts_with(e.work.join(".lmm-drafts/alpha")), "{d:?}");
    assert_eq!(
        fs::read_to_string(e.work.join(".lmm-drafts/.gitignore")).unwrap(),
        "*\n"
    );
    let idx = fs::read_to_string(d.join("index.md")).unwrap();
    assert_eq!(
        idx,
        "---\nlmm_subject: Review result\nlmm_thread_id: t-1\nkind: review\n---\n"
    );
    fs::write(d.join("index.md"), format!("{idx}All good.\n")).unwrap();
    fs::write(d.join("log.txt"), "log").unwrap();
    let (_, p) = e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);
    let posted = fs::read_to_string(p.join("index.md")).unwrap();
    assert!(
        posted.contains(
            "lmm_subject: Review result\nlmm_thread_id: t-1\nkind: review\n---\nAll good.\n"
        ),
        "{posted}"
    );
    assert!(p.join("log.txt").exists());
    assert!(d.exists(), "draft is kept");

    // Empty draft at an explicit DIR.
    let out = e.ok(None, &["draft", "explicit"]);
    let d2 = e.work.join("explicit");
    assert_eq!(out, format!("{}\n", d2.display()));
    assert_eq!(fs::read(d2.join("index.md")).unwrap(), b"");
    assert_eq!(code(&e.run(None, &["draft", "explicit"])), 1);
    // DIR whose parent is .lmm-drafts gets the .gitignore there.
    e.ok(None, &["draft", "x/.lmm-drafts/mine"]);
    assert!(e.work.join("x/.lmm-drafts/.gitignore").exists());
}

#[test]
fn draft_errors_and_locations() {
    let e = setup();
    // Default location needs LMM_NAME.
    assert_eq!(code(&e.run(None, &["draft"])), 2);
    // Inside the data directory.
    let inside = e.home.join("drafts/x");
    assert_eq!(code(&e.run(None, &["draft", inside.to_str().unwrap()])), 2);
    assert!(!inside.exists());
    // LMM_DRAFT_DIR without {name} works without LMM_NAME.
    let o = e
        .cmd(None)
        .env("LMM_DRAFT_DIR", e.work.join("dd"))
        .arg("draft")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", show(&o));
    assert!(
        String::from_utf8_lossy(&o.stdout).starts_with(&e.work.join("dd").display().to_string())
    );
    // draft.dir from config with placeholders.
    fs::write(
        e.work.join(".lmm.yml"),
        "root: true\ndraft:\n  dir: \"{config_dir}/drafts-{name}\"\n",
    )
    .unwrap();
    let out = e.ok(Some("beta"), &["draft"]);
    assert!(
        out.starts_with(&e.work.join("drafts-beta").display().to_string()),
        "{out}"
    );
    // Bad options.
    assert_eq!(code(&e.run(Some("beta"), &["draft", "-m", "lmm_x=1"])), 2);
    assert_eq!(code(&e.run(Some("beta"), &["draft", "--thread", ""])), 2);
}
