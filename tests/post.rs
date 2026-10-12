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
fn post_delivers_with_attachments_and_sent_copy() {
    let e = setup();
    let d = e.msg_dir(
        "m",
        "---\nlmm_subject: Task complete\nlmm_to: ignored\ntags: [report]\n---\nSee ![s](./screen.png).\n",
        &[("screen.png", b"PNG")],
    );
    let before = fs::read(d.join("index.md")).unwrap();
    let (id, path) = e.post(
        "alpha",
        &["-t", "beta", "-m", "task_id=42", d.to_str().unwrap()],
    );
    assert!(path.starts_with(e.new_dir("beta")), "{path:?}");
    let name = path.file_name().unwrap().to_str().unwrap().to_string();
    assert!(
        name.ends_with(&format!("_alpha_to_beta_{}", &id[..8])),
        "{name}"
    );
    assert_eq!(fs::read(path.join("screen.png")).unwrap(), b"PNG");
    let index = fs::read_to_string(path.join("index.md")).unwrap();
    assert!(index.starts_with("---\nlmm_id: "), "{index}");
    assert!(
        index.contains("lmm_from: alpha\nlmm_to: beta\nlmm_created_at: \""),
        "{index}"
    );
    assert!(index.contains("lmm_subject: Task complete\ntags:\n  - report\ntask_id: \"42\"\n---\nSee ![s](./screen.png).\n"), "{index}");
    // Sent copy with the same content.
    let sent = e.sent_dir("alpha").join(&name);
    assert_eq!(fs::read(sent.join("index.md")).unwrap(), index.as_bytes());
    assert!(sent.join("screen.png").exists());
    // Input untouched, work directories cleaned.
    assert_eq!(fs::read(d.join("index.md")).unwrap(), before);
    assert!(entries(&e.tmp_dir("beta")).is_empty());
    assert!(entries(&e.tmp_dir("alpha")).is_empty());
}

#[test]
fn post_errors() {
    let e = setup();
    let d = e.msg_dir("m", "hi\n", &[]);
    let ds = d.to_str().unwrap();
    // LMM_NAME unset / invalid.
    assert_eq!(code(&e.run(None, &["post", "-t", "beta", ds])), 2);
    assert_eq!(code(&e.run(Some("Bad"), &["post", "-t", "beta", ds])), 2);
    // Sender mailbox missing.
    assert_eq!(code(&e.run(Some("nobody"), &["post", "-t", "beta", ds])), 1);
    // Destination missing: error mentions `lmm box create`.
    let o = e.run(Some("alpha"), &["post", "-t", "gamma", ds]);
    assert_eq!(code(&o), 1);
    assert!(String::from_utf8_lossy(&o.stderr).contains("lmm box create gamma"));
    assert!(!e.data_dir.join("boxes/gamma").exists());
    // Invalid destination address.
    assert_eq!(code(&e.run(Some("alpha"), &["post", "-t", "../x", ds])), 2);
    // Multiple -t.
    assert_eq!(
        code(&e.run(Some("alpha"), &["post", "-t", "beta", "-t", "alpha", ds])),
        2
    );
    // File or stdin instead of a directory.
    let f = d.join("index.md");
    assert_eq!(
        code(&e.run(Some("alpha"), &["post", "-t", "beta", f.to_str().unwrap()])),
        2
    );
    assert_eq!(code(&e.run(Some("alpha"), &["post", "-t", "beta", "-"])), 2);
    // No destination.
    assert_eq!(code(&e.run(Some("alpha"), &["post", ds])), 2);
    // Bad meta keys.
    assert_eq!(
        code(&e.run(Some("alpha"), &["post", "-t", "beta", "-m", "lmm_x=1", ds])),
        2
    );
    assert_eq!(
        code(&e.run(Some("alpha"), &["post", "-t", "beta", "-m", "novalue", ds])),
        2
    );
    // Bad thread.
    let long = "x".repeat(129);
    assert_eq!(
        code(&e.run(
            Some("alpha"),
            &["post", "-t", "beta", "--thread", &long, ds]
        )),
        2
    );
    // Invalid message directories.
    let sub = e.msg_dir("withsub", "x", &[]);
    fs::create_dir(sub.join("dir")).unwrap();
    assert_eq!(
        code(&e.run(
            Some("alpha"),
            &["post", "-t", "beta", sub.to_str().unwrap()]
        )),
        1
    );
    let noindex = e.work.join("noindex");
    fs::create_dir(&noindex).unwrap();
    assert_eq!(
        code(&e.run(
            Some("alpha"),
            &["post", "-t", "beta", noindex.to_str().unwrap()]
        )),
        1
    );
    let case = e.msg_dir("case", "x", &[("a.txt", b"1"), ("A.txt", b"2")]);
    // Only a case-sensitive file system can hold both names.
    if entries(&case).len() == 3 {
        assert_eq!(
            code(&e.run(
                Some("alpha"),
                &["post", "-t", "beta", case.to_str().unwrap()]
            )),
            1
        );
    }
    // Bad front matter.
    let bad = e.msg_dir("bad", "---\nlmm_thread_id: 42\n---\n", &[]);
    assert_eq!(
        code(&e.run(
            Some("alpha"),
            &["post", "-t", "beta", bad.to_str().unwrap()]
        )),
        1
    );
    let unclosed = e.msg_dir("unclosed", "---\na: 1\n", &[]);
    assert_eq!(
        code(&e.run(
            Some("alpha"),
            &["post", "-t", "beta", unclosed.to_str().unwrap()]
        )),
        1
    );
    assert!(entries(&e.new_dir("beta")).is_empty());
}

#[test]
fn post_size_limit() {
    let e = setup();
    let cfg = e.tmp.path().join("global.yml");
    fs::write(&cfg, "max_message_size: 100\n").unwrap();
    let d = e.msg_dir("m", "hi\n", &[("big.bin", &[0u8; 200])]);
    let o = e
        .cmd(Some("alpha"))
        .env("LMM_CONFIG", &cfg)
        .args(["post", "-t", "beta", d.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(code(&o), 1, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("too large"));
}

#[test]
fn post_to_self_and_name_collisions() {
    let e = setup();
    let d = e.msg_dir("m", "hi\n", &[]);
    let (_, p) = e.post("alpha", &["-t", "alpha", d.to_str().unwrap()]);
    assert!(p.starts_with(e.new_dir("alpha")));
    assert_eq!(entries(&e.sent_dir("alpha")).len(), 1);
    // Duplicates in new/ and cur/ force suffixes.
    let name = p.file_name().unwrap().to_owned();
    fs::create_dir(e.cur_dir("beta").join(&name)).unwrap();
    fs::create_dir(e.new_dir("beta").join(&name)).unwrap();
    let mut n1 = name.clone();
    n1.push("_1");
    fs::create_dir(e.cur_dir("beta").join(&n1)).unwrap();
    // Use the library to publish under an existing name.
    let mut n2 = name.clone();
    n2.push("_2");
    let src = e.tmp.path().join("src");
    fs::create_dir(&src).unwrap();
    let new = e.new_dir("beta");
    let cur = e.cur_dir("beta");
    let got = lmm::fsutil::rename_unique(&src, &new, &name, |n| cur.join(n).exists()).unwrap();
    assert_eq!(got, new.join(n2));
}

#[test]
fn reply_sets_values() {
    let e = setup();
    let d = e.msg_dir("m", "---\nlmm_subject: Review\n---\nplease\n", &[]);
    let (id, p) = e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);
    let r = e.msg_dir("r", "done\n", &[]);
    let (_, rp) = e.post(
        "beta",
        &["--reply", p.to_str().unwrap(), r.to_str().unwrap()],
    );
    assert!(rp.starts_with(e.new_dir("alpha")));
    let idx = fs::read_to_string(rp.join("index.md")).unwrap();
    assert!(idx.contains("lmm_from: beta\nlmm_to: alpha\n"), "{idx}");
    assert!(idx.contains("lmm_subject: \"Re: Review\"\n"), "{idx}");
    assert!(
        idx.contains(&format!("lmm_thread_id: {}\n", yaml_str(&id))),
        "{idx}"
    );
    assert!(
        idx.contains(&format!("lmm_in_reply_to: {}\n", yaml_str(&id))),
        "{idx}"
    );
    // Reply to the reply via its index.md keeps a single "Re: " and the thread.
    let r2 = e.msg_dir("r2", "ok\n", &[]);
    let (_, rp2) = e.post(
        "alpha",
        &[
            "-r",
            rp.join("index.md").to_str().unwrap(),
            r2.to_str().unwrap(),
        ],
    );
    let idx2 = fs::read_to_string(rp2.join("index.md")).unwrap();
    assert!(idx2.contains("lmm_subject: \"Re: Review\"\n"), "{idx2}");
    assert!(
        idx2.contains(&format!("lmm_thread_id: {}\n", yaml_str(&id))),
        "{idx2}"
    );
    // Invalid reply source.
    let bad = e.msg_dir("badsrc", "---\nlmm_from: Beta\nlmm_id: x\n---\n", &[]);
    assert_eq!(
        code(&e.run(
            Some("alpha"),
            &["post", "-r", bad.to_str().unwrap(), r.to_str().unwrap()]
        )),
        1
    );
    // --reply and --forward conflict.
    assert_eq!(
        code(&e.run(
            Some("alpha"),
            &[
                "post",
                "--forward",
                "-r",
                p.to_str().unwrap(),
                r.to_str().unwrap()
            ]
        )),
        2
    );
}

fn yaml_str(s: &str) -> String {
    lmm::yaml::emit_str(s)
}

#[test]
fn forward_chain_records_history() {
    let e = setup();
    e.create_box("gamma");
    e.create_box("delta");
    let d = e.msg_dir(
        "m",
        "---\nlmm_subject: S\nlmm_thread_id: task-042\nk: v\n---\nbody\n",
        &[("a.txt", b"A")],
    );
    let (id1, p1) = e.post("alpha", &["-t", "beta", d.to_str().unwrap()]);
    let (id2, p2) = e.post("beta", &["--forward", "-t", "gamma", p1.to_str().unwrap()]);
    let (_, p3) = e.post("gamma", &["--forward", "-t", "delta", p2.to_str().unwrap()]);
    let out = e.ok(Some("delta"), &["show", "--json", p3.to_str().unwrap()]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let fm = &v["frontmatter"];
    assert_eq!(fm["lmm_from"], "gamma");
    assert_eq!(fm["lmm_to"], "delta");
    assert_eq!(fm["lmm_subject"], "S");
    assert_eq!(fm["lmm_thread_id"], "task-042");
    assert_eq!(fm["k"], "v");
    let h = fm["lmm_forwarded"].as_array().unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!(h[0]["id"], id1.as_str());
    assert_eq!(h[0]["from"], "alpha");
    assert_eq!(h[0]["to"], "beta");
    assert_eq!(h[1]["id"], id2.as_str());
    assert_eq!(h[1]["from"], "beta");
    assert_eq!(h[1]["to"], "gamma");
    assert_eq!(v["attachments"], serde_json::json!(["a.txt"]));
    // Without --forward, history is dropped.
    let (_, p4) = e.post("delta", &["-t", "alpha", p3.to_str().unwrap()]);
    let idx = fs::read_to_string(p4.join("index.md")).unwrap();
    assert!(!idx.contains("lmm_forwarded"), "{idx}");
    // --forward on a non-received message fails.
    assert_eq!(
        code(&e.run(
            Some("alpha"),
            &["post", "--forward", "-t", "beta", d.to_str().unwrap()]
        )),
        1
    );
    // Malformed history fails.
    let bad = e.msg_dir(
        "badfwd",
        &received_index(
            "a1b2c3d4-e5f6-4890-abcd-ef1234567890",
            "alpha",
            "beta",
            "lmm_forwarded:\n  - id: x\n",
        ),
        &[],
    );
    assert_eq!(
        code(&e.run(
            Some("beta"),
            &["post", "--forward", "-t", "alpha", bad.to_str().unwrap()]
        )),
        1
    );
}

#[test]
fn forward_history_limit() {
    let e = setup();
    let mut extra = String::from("lmm_forwarded:\n");
    for _ in 0..100 {
        extra.push_str("  - id: a1b2c3d4-e5f6-4890-abcd-ef1234567890\n    from: alpha\n    to: beta\n    created_at: \"2026-10-05T14:30:05.123456Z\"\n");
    }
    let d = e.msg_dir(
        "m",
        &received_index(
            "a1b2c3d4-e5f6-4890-abcd-ef1234567890",
            "alpha",
            "beta",
            &extra,
        ),
        &[],
    );
    let o = e.run(
        Some("beta"),
        &["post", "--forward", "-t", "alpha", d.to_str().unwrap()],
    );
    assert_eq!(code(&o), 1, "{}", show(&o));
    assert!(String::from_utf8_lossy(&o.stderr).contains("100"));
}

#[test]
fn reply_to_and_meta_from_config() {
    let e = setup();
    fs::write(
        e.work.join(".lmm.yml"),
        "root: true\npost:\n  reply_to: beta\n  meta:\n    project: myrepo\n    who: \"{name}\"\n",
    )
    .unwrap();
    let d = e.msg_dir("m", "---\nproject: override\n---\nx\n", &[]);
    let (_, p) = e.post("alpha", &[d.to_str().unwrap()]);
    assert!(p.starts_with(e.new_dir("beta")));
    let idx = fs::read_to_string(p.join("index.md")).unwrap();
    assert!(idx.contains("project: override\nwho: alpha\n"), "{idx}");
    let (_, p) = e.post("alpha", &["-m", "project=cli", d.to_str().unwrap()]);
    let idx = fs::read_to_string(p.join("index.md")).unwrap();
    assert!(idx.contains("project: cli\n"), "{idx}");
}
