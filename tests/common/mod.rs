#![allow(dead_code)]

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

pub struct Repo {
    temp: TempDir,
}

impl Repo {
    pub fn new() -> Self {
        let repo = Self {
            temp: tempfile::tempdir().unwrap(),
        };
        fs::create_dir(repo.root().join("home")).unwrap();
        fs::write(
            repo.root().join("config.toml"),
            r#"
[user]
name = "Integration Test"
email = "integration@example.invalid"
[ui]
color = "never"
paginate = "never"
editor = "false"
[signing]
behavior = "drop"
"#,
        )
        .unwrap();
        let version = repo.command("jj").arg("--version").output().expect(
            "jj integration tests require jj on PATH; install jj before running cargo test",
        );
        success(&["jj", "--version"], version);
        repo.jj(&["git", "init", "--colocate", "repo"]);
        repo.jj(&[
            "config",
            "set",
            "--repo",
            "revset-aliases.\"trunk()\"",
            "bookmarks(exact:trunk)",
        ]);
        fs::write(repo.path().join("trunk.txt"), "trunk\n").unwrap();
        repo.jj(&["describe", "-m", "trunk"]);
        repo.jj(&["bookmark", "set", "trunk", "-r", "@"]);
        repo
    }

    pub fn root(&self) -> &Path {
        self.temp.path()
    }

    pub fn path(&self) -> std::path::PathBuf {
        self.root().join("repo")
    }

    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut cmd = Command::new(program);
        // Apply to each subprocess, including the tools binary whose jj children
        // inherit this environment. Never change the test process's environment.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("JJ_") || key.to_string_lossy().starts_with("GIT_")
            {
                cmd.env_remove(key);
            }
        }
        cmd.env("HOME", self.root().join("home"))
            .env("XDG_CONFIG_HOME", self.root().join("home"))
            .env("JJ_CONFIG", self.root().join("config.toml"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(if self.path().exists() {
                self.path()
            } else {
                self.root().to_path_buf()
            });
        let mut paths = vec![self.root().join("bin")];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        cmd.env("PATH", std::env::join_paths(paths).unwrap());
        cmd
    }

    #[cfg(unix)]
    pub fn install_stage_alias(&self) {
        fs::create_dir(self.root().join("bin")).unwrap();
        std::os::unix::fs::symlink(
            env!("CARGO_BIN_EXE_tools"),
            self.root().join("bin/jj-stack"),
        )
        .unwrap();
        let config = self.root().join("config.toml");
        let original = fs::read_to_string(&config).unwrap();
        let alias = r#"
[aliases]
stage = [
  "util", "exec", "--", "jj-stack", "-r",
  '''
  coalesce(
    heads(::@ & merges() & mutable() & (trunk()..) & description(regex:"^megamerge\n?$")),
    trunk(),
  )+:: & mine() & mutable() ~ empty()
  ''',
]
"#;
        fs::write(config, format!("{original}\n{alias}")).unwrap();
    }

    pub fn jj(&self, args: &[&str]) -> String {
        success(args, self.command("jj").args(args).output().unwrap())
    }

    pub fn stack(&self, revision: &str) -> Output {
        self.command(env!("CARGO_BIN_EXE_tools"))
            .args(["jj-stack", "-r", revision])
            .output()
            .unwrap()
    }

    pub fn stage(&self, revision: &str) {
        success(&["jj-stack", "-r", revision], self.stack(revision));
    }

    pub fn ids(&self, revset: &str) -> BTreeSet<String> {
        self.jj(&[
            "log",
            "--no-graph",
            "-r",
            revset,
            "-T",
            "change_id ++ \"\\n\"",
        ])
        .lines()
        .map(String::from)
        .collect()
    }

    pub fn id(&self, revset: &str) -> String {
        let ids = self.ids(revset);
        assert_eq!(ids.len(), 1, "expected one change for {revset}: {ids:?}");
        ids.into_iter().next().unwrap()
    }

    pub fn parents(&self, revision: &str) -> BTreeSet<String> {
        self.ids(&format!("({revision})-"))
    }

    pub fn assert_megamerge_description(&self, revision: &str) {
        assert_eq!(
            self.jj(&[
                "log",
                "--no-graph",
                "-r",
                revision,
                "-T",
                "description.first_line()",
            ]),
            "megamerge"
        );
        assert_eq!(
            self.ids("description(substring:megamerge)"),
            set(&[revision])
        );
    }

    pub fn change(&self, parent: &str, name: &str) -> String {
        self.jj(&["new", parent, "-m", name]);
        fs::write(self.path().join(format!("{name}.txt")), format!("{name}\n")).unwrap();
        self.id("@")
    }

    pub fn merge(&self, parents: &[&str], description: &str) -> String {
        let mut args = vec!["new"];
        args.extend_from_slice(parents);
        args.extend(["-m", description]);
        self.jj(&args);
        let id = self.id("@");
        self.jj(&["new"]);
        id
    }

    pub fn snapshot(&self) -> (BTreeSet<String>, String) {
        let commits = self
            .jj(&[
                "log",
                "--no-graph",
                "-r",
                "all()",
                "-T",
                "commit_id ++ \"\\n\"",
            ])
            .lines()
            .map(String::from)
            .collect();
        (commits, self.id("@"))
    }
}

pub fn success(args: &[&str], output: Output) -> String {
    assert!(
        output.status.success(),
        "command {args:?} failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).unwrap()
}

pub fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}
