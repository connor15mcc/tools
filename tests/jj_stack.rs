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

// Characterization of the broad current heuristic; change this expectation
// when merge detection is tightened as described in jj-stack-land-plan.md.
#[test]
fn currently_treats_semantic_merge_as_megamerge() {
    existing_merge_receives_change("semantic merge");
}

fn fallback_parallelizes_chain(select_unrelated: bool) {
    let repo = Repo::new();
    let trunk = repo.id("trunk()");
    let c = repo.change("trunk", "C");
    let a = repo.change("trunk", "A");
    let b = repo.change(&a, "B");
    repo.jj(&["new"]);
    let former_working_copy = repo.id("@");
    let selection = if select_unrelated {
        c.clone()
    } else {
        format!("{a} | {b}")
    };

    repo.stage(&selection);

    for revision in [&a, &b, &c] {
        assert_eq!(repo.parents(revision), set(&[&trunk]));
    }
    repo.assert_megamerge_description(&former_working_copy);
    assert_eq!(repo.parents(&former_working_copy), set(&[&a, &b]));
    assert_eq!(repo.ids("merges()"), set(&[&former_working_copy]));
    assert_ne!(repo.id("@"), former_working_copy);
    assert_eq!(repo.parents("@"), set(&[&former_working_copy]));
    assert_eq!(repo.ids("@ & empty()"), repo.ids("@"));
}

#[test]
fn fallback_parallelizes_ancestors_and_commits_working_copy_as_megamerge() {
    fallback_parallelizes_chain(false);
}

// Known limitation: the fallback uses trunk()..@- instead of --revision.
#[test]
fn fallback_currently_ignores_selected_revision() {
    fallback_parallelizes_chain(true);
}

// Known limitation: a change named megamerge is created even with one parent.
#[test]
fn fallback_currently_creates_single_parent_megamerge() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    repo.jj(&["new"]);
    let former_working_copy = repo.id("@");

    repo.stage(&a);

    repo.assert_megamerge_description(&former_working_copy);
    assert_eq!(repo.parents(&former_working_copy), set(&[&a]));
    assert!(repo.ids("merges()").is_empty());
    assert_ne!(repo.id("@"), former_working_copy);
    assert_eq!(repo.parents("@"), set(&[&former_working_copy]));
    assert_eq!(repo.ids("@ & empty()"), repo.ids("@"));
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
