use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

struct Repo {
    temp: TempDir,
}

impl Repo {
    fn new() -> Self {
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
[signing]
behavior = "drop"
"#,
        )
        .unwrap();
        let version = repo.command("jj").arg("--version").output().expect(
            "jj-stack integration tests require jj on PATH; install jj before running cargo test",
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

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn path(&self) -> std::path::PathBuf {
        self.root().join("repo")
    }

    fn command(&self, program: impl AsRef<OsStr>) -> Command {
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
        cmd
    }

    fn jj(&self, args: &[&str]) -> String {
        success(args, self.command("jj").args(args).output().unwrap())
    }

    fn stack(&self, revision: &str) -> Output {
        self.command(env!("CARGO_BIN_EXE_tools"))
            .args(["jj-stack", "-r", revision])
            .output()
            .unwrap()
    }

    fn stage(&self, revision: &str) {
        success(&["jj-stack", "-r", revision], self.stack(revision));
    }

    fn ids(&self, revset: &str) -> BTreeSet<String> {
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

    fn id(&self, revset: &str) -> String {
        let ids = self.ids(revset);
        assert_eq!(ids.len(), 1, "expected one change for {revset}: {ids:?}");
        ids.into_iter().next().unwrap()
    }

    fn parents(&self, revision: &str) -> BTreeSet<String> {
        self.ids(&format!("({revision})-"))
    }

    fn assert_megamerge_description(&self, revision: &str) {
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

    fn change(&self, parent: &str, name: &str) -> String {
        self.jj(&["new", parent, "-m", name]);
        fs::write(self.path().join(format!("{name}.txt")), format!("{name}\n")).unwrap();
        self.id("@")
    }

    fn merge(&self, parents: &[&str], description: &str) -> String {
        let mut args = vec!["new"];
        args.extend_from_slice(parents);
        args.extend(["-m", description]);
        self.jj(&args);
        let id = self.id("@");
        self.jj(&["new"]);
        id
    }

    fn snapshot(&self) -> (BTreeSet<String>, String) {
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

fn success(args: &[&str], output: Output) -> String {
    assert!(
        output.status.success(),
        "command {args:?} failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).unwrap()
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn existing_merge_receives_change(description: &str) {
    let repo = Repo::new();
    let trunk = repo.id("trunk()");
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let c = repo.change("trunk", "C");
    let merge = repo.merge(&[&a, &b], description);
    let working_copy = repo.id("@");

    repo.stage(&c);

    assert_eq!(repo.parents(&merge), set(&[&a, &b, &c]));
    for revision in [&a, &b, &c] {
        assert_eq!(repo.parents(revision), set(&[&trunk]));
    }
    assert_eq!(repo.id("@"), working_copy);
    assert_eq!(repo.parents("@"), set(&[&merge]));
    assert_eq!(repo.ids("merges()"), set(&[&merge]));
}

#[test]
fn stages_change_into_existing_megamerge() {
    existing_merge_receives_change("megamerge");
}

#[test]
fn leaves_semantic_merges_untouched() {
    for description in ["semantic merge", "megamerge extra", "megamerge\n\nDetails"] {
        let repo = Repo::new();
        let a = repo.change("trunk", "A");
        let b = repo.change("trunk", "B");
        let c = repo.change("trunk", "C");
        repo.merge(&[&a, &b], description);
        let before = repo.snapshot();

        repo.stage(&c);

        assert_eq!(
            repo.snapshot(),
            before,
            "merge description: {description:?}"
        );
    }
}

#[test]
fn parallelizes_selected_chain_and_creates_megamerge() {
    let repo = Repo::new();
    let trunk = repo.id("trunk()");
    let a = repo.change("trunk", "A");
    let b = repo.change(&a, "B");
    repo.jj(&["new"]);
    let former_working_copy = repo.id("@");

    // This graph-dependent revset must be resolved before parallelization.
    repo.stage("trunk()..@-");

    for revision in [&a, &b] {
        assert_eq!(repo.parents(revision), set(&[&trunk]));
    }
    let merge = repo.id("merges() & description(substring:megamerge)");
    repo.assert_megamerge_description(&merge);
    assert_eq!(repo.parents(&merge), set(&[&a, &b]));
    assert_ne!(merge, former_working_copy);
    // jj abandons the old empty, undescribed working copy when creating the
    // merge. A fresh empty child is the only working copy we need to retain.
    assert_ne!(repo.id("@"), former_working_copy);
    assert_eq!(repo.parents("@"), set(&[&merge]));
    assert_eq!(repo.ids("@ & empty()"), repo.ids("@"));
}

#[test]
fn selected_siblings_create_megamerge_and_preserve_unselected_work() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let c = repo.change("trunk", "C");
    // Leave real uncommitted work in @; creating the merge must preserve it.
    fs::write(repo.path().join("pending.txt"), "pending\n").unwrap();
    let c_commit = repo.jj(&["log", "--no-graph", "-r", "@", "-T", "commit_id"]);

    repo.stage(&format!("{a} | {b}"));

    let merge = repo.id("merges() & description(substring:megamerge)");
    repo.assert_megamerge_description(&merge);
    assert_eq!(repo.parents(&merge), set(&[&a, &b]));
    assert_eq!(repo.parents("@"), set(&[&merge]));
    assert_eq!(repo.ids("@ & empty()"), repo.ids("@"));
    assert_eq!(
        repo.jj(&["log", "--no-graph", "-r", &c, "-T", "commit_id"]),
        c_commit
    );
    assert_eq!(
        repo.jj(&["file", "show", "-r", &c, "pending.txt"]),
        "pending\n"
    );
    assert!(!repo.path().join("pending.txt").exists());
}

#[test]
fn selected_working_copy_is_preserved_as_one_of_three_parents() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let c = repo.change("trunk", "C");

    repo.stage(&format!("{a} | {b} | @"));

    let merge = repo.id("merges() & description(substring:megamerge)");
    repo.assert_megamerge_description(&merge);
    assert_eq!(repo.parents(&merge), set(&[&a, &b, &c]));
    assert_eq!(repo.jj(&["file", "show", "-r", &c, "C.txt"]), "C\n");
    assert_eq!(repo.parents("@"), set(&[&merge]));
    assert_eq!(repo.ids("@ & empty()"), repo.ids("@"));
}

#[test]
fn single_selected_change_leaves_unrelated_chain_untouched() {
    let repo = Repo::new();
    let c = repo.change("trunk", "C");
    let a = repo.change("trunk", "A");
    repo.change(&a, "B");
    repo.jj(&["new"]);
    let before = repo.snapshot();

    repo.stage(&c);

    assert_eq!(repo.snapshot(), before);
    assert!(repo.ids("description(substring:megamerge)").is_empty());
}

#[test]
fn single_selected_change_does_not_create_megamerge() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    repo.jj(&["new"]);
    let before = repo.snapshot();

    repo.stage(&a);

    assert_eq!(repo.snapshot(), before);
    assert!(repo.ids("merges()").is_empty());
    assert!(repo.ids("description(substring:megamerge)").is_empty());
}

#[test]
fn ignores_immutable_megamerge() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let c = repo.change("trunk", "C");
    let merge = repo.merge(&[&a, &b], "megamerge");
    repo.jj(&[
        "config",
        "set",
        "--repo",
        "revset-aliases.\"immutable_heads()\"",
        &merge,
    ]);
    let before = repo.snapshot();

    repo.stage(&c);

    assert_eq!(repo.snapshot(), before);
}

#[test]
fn ambiguous_active_megamerges_fail_before_mutating() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let c = repo.change("trunk", "C");
    let first = repo.merge(&[&a, &b], "megamerge");
    let second = repo.merge(&[&a, &c], "megamerge");
    repo.merge(&[&first, &second], "semantic merge");
    let before = repo.snapshot();

    let output = repo.stack(&c);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("multiple active megamerges"));
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn invalid_revisions_fail_without_changing_repository() {
    let repo = Repo::new();
    repo.change("trunk", "A");
    let before = repo.snapshot();

    for (revision, diagnostic) in [
        ("", "--revision must not be empty"),
        (" \t\n ", "--revision must not be empty"),
        ("none()", "no commits match revset"),
        ("(", "Syntax error"),
    ] {
        let output = repo.stack(revision);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success() && stderr.contains(diagnostic),
            "jj-stack -r {revision:?}: status {}\nstdout:\n{}\nstderr:\n{stderr}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
        );
        assert_eq!(
            repo.snapshot(),
            before,
            "repository changed for {revision:?}"
        );
    }
}
