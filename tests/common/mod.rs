//! Shared helpers for integration tests.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

pub const BIN: &str = env!("CARGO_BIN_EXE_lmm");

/// An isolated environment: user home, data directory and work directory.
pub struct Env {
    pub tmp: TempDir,
    pub user_home: PathBuf,
    pub home: PathBuf,
    pub work: PathBuf,
}

impl Env {
    pub fn new() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let user_home = root.join("user");
        let home = root.join("lmmhome");
        let work = root.join("work");
        fs::create_dir_all(&user_home).unwrap();
        fs::create_dir_all(&work).unwrap();
        // Stop config discovery at the work directory.
        fs::write(work.join(".lmm.yml"), "root: true\n").unwrap();
        Env {
            tmp,
            user_home,
            home,
            work,
        }
    }

    /// A command running as `name` (or without LMM_NAME).
    pub fn cmd(&self, name: Option<&str>) -> Command {
        let mut c = Command::new(BIN);
        for (k, _) in std::env::vars() {
            if k.starts_with("LMM_") {
                c.env_remove(k);
            }
        }
        c.env("HOME", &self.user_home)
            .env("LMM_HOME", &self.home)
            .current_dir(&self.work)
            .stdin(Stdio::null());
        if let Some(n) = name {
            c.env("LMM_NAME", n);
        }
        c
    }

    pub fn run(&self, name: Option<&str>, args: &[&str]) -> Output {
        self.cmd(name).args(args).output().unwrap()
    }

    pub fn ok(&self, name: Option<&str>, args: &[&str]) -> String {
        let o = self.run(name, args);
        assert!(o.status.success(), "lmm {args:?} failed: {}", show(&o));
        String::from_utf8(o.stdout).unwrap()
    }

    pub fn create_box(&self, addr: &str) {
        self.ok(None, &["box", "create", addr]);
    }

    /// Make a message directory under the work directory.
    pub fn msg_dir(&self, rel: &str, index: &str, attachments: &[(&str, &[u8])]) -> PathBuf {
        let d = self.work.join(rel);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("index.md"), index).unwrap();
        for (n, data) in attachments {
            fs::write(d.join(n), data).unwrap();
        }
        d
    }

    /// Post and return (id, path).
    pub fn post(&self, from: &str, args: &[&str]) -> (String, PathBuf) {
        let mut a = vec!["post"];
        a.extend_from_slice(args);
        let out = self.ok(Some(from), &a);
        let line = out.strip_suffix('\n').expect("newline-terminated output");
        assert!(!line.contains('\n'), "single line: {out:?}");
        let (id, path) = line.split_once(' ').unwrap();
        (id.to_string(), PathBuf::from(path))
    }

    pub fn new_dir(&self, addr: &str) -> PathBuf {
        self.home.join("boxes").join(addr).join("inbox/new")
    }

    pub fn cur_dir(&self, addr: &str) -> PathBuf {
        self.home.join("boxes").join(addr).join("inbox/cur")
    }

    pub fn failed_dir(&self, addr: &str) -> PathBuf {
        self.home.join("boxes").join(addr).join("inbox/failed")
    }

    pub fn sent_dir(&self, addr: &str) -> PathBuf {
        self.home.join("boxes").join(addr).join("sent")
    }

    pub fn tmp_dir(&self, addr: &str) -> PathBuf {
        self.home.join("boxes").join(addr).join("inbox/.tmp")
    }
}

pub fn show(o: &Output) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

pub fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

pub fn entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = match fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

pub fn lines(s: &str) -> Vec<String> {
    s.lines().map(str::to_string).collect()
}

/// Poll `cond` every 10 ms until it holds; panic after 10 seconds.
pub fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !cond() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

impl Env {
    /// Wait until a sender has written `msg/index.md` into a work directory
    /// of `addr` (it then holds the work directory lock and is about to take,
    /// or is waiting for, the mailbox lock).
    pub fn wait_for_workdir(&self, addr: &str) {
        let tmp = self.tmp_dir(addr);
        wait_until("a work directory with msg/index.md", || {
            fs::read_dir(&tmp)
                .map(|rd| rd.flatten().any(|e| e.path().join("msg/index.md").exists()))
                .unwrap_or(false)
        });
    }

    /// Wait until `lmm wait`/`lmm hook` created its receive directory.
    pub fn wait_for_dir(&self, p: &Path) {
        wait_until(&format!("{}", p.display()), || p.is_dir());
    }
}

/// A syntactically valid received message front matter.
pub fn received_index(id: &str, from: &str, to: &str, extra: &str) -> String {
    format!(
        "---\nlmm_id: {id}\nlmm_from: {from}\nlmm_to: {to}\nlmm_created_at: \"2026-10-05T14:30:05.123456Z\"\n{extra}---\nbody\n"
    )
}
