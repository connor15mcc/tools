mod common;

use common::{set, success, Repo};
use std::process::Output;

fn land(repo: &Repo, revision: &str, extra: &[&str]) -> Output {
    repo.command(env!("CARGO_BIN_EXE_tools"))
        .args(["jj-land", "-r", revision])
        .args(extra)
        .output()
        .unwrap()
}

fn fails(repo: &Repo, revision: &str, extra: &[&str], message: &str) {
    let before = repo.snapshot();
    let bookmark_template =
        r#"local_bookmarks.map(|b| b.name()).join(",") ++ ":" ++ commit_id ++ "\n""#;
    let bookmarks = repo.jj(&[
        "log",
        "-r",
        "bookmarks()",
        "--no-graph",
        "-T",
        bookmark_template,
    ]);
    let output = land(repo, revision, extra);
    assert!(
        !output.status.success() && String::from_utf8_lossy(&output.stderr).contains(message),
        "jj-land -r {revision:?} {extra:?}: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(repo.snapshot(), before);
    assert_eq!(
        repo.jj(&[
            "log",
            "-r",
            "bookmarks()",
            "--no-graph",
            "-T",
            bookmark_template
        ]),
        bookmarks
    );
}

#[test]
fn two_parent_merge_collapses_and_preserves_working_copy_changes() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    repo.merge(&[&a, &b], "megamerge");
    std::fs::write(repo.path().join("pending.txt"), "pending\n").unwrap();
    let working_copy = repo.id("@");

    success(&["jj-land", "-r", &a], land(&repo, &a, &[]));

    assert_eq!(repo.id("trunk()"), a);
    assert!(repo.ids("merges()").is_empty());
    assert!(repo.ids("description(substring:megamerge)").is_empty());
    assert_eq!(repo.parents(&b), set(&[&a]));
    assert_eq!(repo.parents("@"), set(&[&b]));
    assert_eq!(repo.id("@"), working_copy);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("pending.txt")).unwrap(),
        "pending\n"
    );
    assert_eq!(repo.jj(&["file", "show", "-r", &b, "A.txt"]), "A\n");
    assert_eq!(repo.jj(&["file", "show", "-r", &b, "B.txt"]), "B\n");
}

#[test]
fn three_parent_merge_keeps_remaining_branches_and_can_land_again() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let c = repo.change("trunk", "C");
    let merge = repo.merge(&[&a, &b, &c], "megamerge");
    let working_copy = repo.id("@");

    success(&["jj-land", "-r", &a], land(&repo, &a, &[]));

    assert_eq!(repo.id("trunk()"), a);
    assert_eq!(repo.parents(&b), set(&[&a]));
    assert_eq!(repo.parents(&c), set(&[&a]));
    assert_eq!(repo.parents(&merge), set(&[&b, &c]));
    repo.assert_megamerge_description(&merge);
    assert_eq!(repo.parents("@"), set(&[&merge]));
    assert_eq!(repo.id("@"), working_copy);

    success(&["jj-land", "-r", &b], land(&repo, &b, &[]));
    assert_eq!(repo.id("trunk()"), b);
    assert_eq!(repo.parents(&c), set(&[&b]));
    assert_eq!(repo.parents("@"), set(&[&c]));
    assert!(repo.ids("merges()").is_empty());
}

#[test]
fn rebases_whole_remaining_branch_and_preserves_its_dependencies() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let base = repo.change("trunk", "Base");
    let tip = repo.change(&base, "Tip");
    repo.merge(&[&a, &tip], "megamerge");

    success(&["jj-land", "-r", &a], land(&repo, &a, &[]));

    assert_eq!(repo.parents(&base), set(&[&a]));
    assert_eq!(repo.parents(&tip), set(&[&base]));
    assert_eq!(repo.parents("@"), set(&[&tip]));
    assert_eq!(repo.jj(&["file", "show", "-r", &tip, "Base.txt"]), "Base\n");
}

#[test]
fn landing_from_megamerge_itself_creates_fresh_working_copy() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let merge = repo.merge(&[&a, &b], "megamerge");
    repo.jj(&["edit", &merge]);

    success(&["jj-land", "-r", &a], land(&repo, &a, &[]));

    assert_eq!(repo.id("trunk()"), a);
    assert_eq!(repo.parents("@"), set(&[&b]));
    assert_eq!(repo.ids("@ & empty()"), repo.ids("@"));
    assert!(repo.ids("merges()").is_empty());
}

#[test]
fn shared_branch_history_is_preserved_when_rebasing_remaining_heads() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let base = repo.change("trunk", "Base");
    let b = repo.change(&base, "B");
    let c = repo.change(&base, "C");
    let merge = repo.merge(&[&a, &b, &c], "megamerge");

    success(&["jj-land", "-r", &a], land(&repo, &a, &[]));

    assert_eq!(repo.parents(&base), set(&[&a]));
    assert_eq!(repo.parents(&b), set(&[&base]));
    assert_eq!(repo.parents(&c), set(&[&base]));
    assert_eq!(repo.parents(&merge), set(&[&b, &c]));
}

#[test]
fn invalid_selections_and_bookmarks_do_not_mutate_graph() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let outside = repo.change("trunk", "Outside");
    repo.merge(&[&a, &b], "megamerge");
    for (revision, message) in [
        ("", "--revision must not be empty"),
        (" \t", "--revision must not be empty"),
        ("none()", "exactly one commit"),
        ("trunk() | @", "exactly one commit"),
        ("(", "Syntax error"),
        (&outside, "not a direct parent"),
    ] {
        fails(&repo, revision, &[], message);
    }
    fails(
        &repo,
        &a,
        &["--bookmark", "missing"],
        "must resolve to one ancestor",
    );
    repo.jj(&["bookmark", "set", "other", "-r", "trunk"]);
    fails(&repo, &a, &[], "cannot infer a single local trunk bookmark");
    success(
        &["jj-land", "-r", &a],
        land(&repo, &a, &["--bookmark", "trunk"]),
    );
    assert_eq!(repo.id("trunk"), a);
    assert_ne!(repo.id("other"), a);
}

#[test]
fn requires_active_mutable_megamerge() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    fails(&repo, &a, &[], "no active megamerge");
    repo.merge(&[&a, &b], "semantic merge");
    fails(&repo, &a, &[], "no active megamerge");
    let merge = repo.merge(&[&a, &b], "megamerge");
    repo.jj(&[
        "config",
        "set",
        "--repo",
        "revset-aliases.\"immutable_heads()\"",
        &merge,
    ]);
    fails(&repo, &a, &[], "no active megamerge");
}

#[test]
fn ambiguous_active_megamerges_fail_before_mutation() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let c = repo.change("trunk", "C");
    let first = repo.merge(&[&a, &b], "megamerge");
    let second = repo.merge(&[&a, &c], "megamerge");
    repo.merge(&[&first, &second], "semantic merge");
    fails(&repo, &a, &[], "multiple active megamerges");
}

#[test]
fn refuses_to_discard_megamerge_edits() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let merge = repo.merge(&[&a, &b], "megamerge");
    repo.jj(&["edit", &merge]);
    std::fs::write(repo.path().join("merge-edits.txt"), "keep me\n").unwrap();
    repo.jj(&["new"]);
    fails(&repo, &a, &[], "active megamerge has changes");
}

#[test]
fn preserves_megamerge_edits_when_merge_survives() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let c = repo.change("trunk", "C");
    let merge = repo.merge(&[&a, &b, &c], "megamerge");
    repo.jj(&["edit", &merge]);
    std::fs::write(repo.path().join("merge-edits.txt"), "keep me\n").unwrap();
    repo.jj(&["new"]);

    success(&["jj-land", "-r", &a], land(&repo, &a, &[]));

    assert_eq!(repo.id("trunk()"), a);
    assert_eq!(repo.parents(&merge), set(&[&b, &c]));
    repo.assert_megamerge_description(&merge);
    assert_eq!(
        repo.jj(&["file", "show", "-r", &merge, "merge-edits.txt"]),
        "keep me\n"
    );
}

#[test]
fn immutable_remaining_branches_fail_before_bookmark_advances() {
    let repo = Repo::new();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let published = repo.change(&b, "Published");
    repo.merge(&[&a, &b], "megamerge");
    repo.jj(&[
        "config",
        "set",
        "--repo",
        "revset-aliases.\"immutable_heads()\"",
        &published,
    ]);

    fails(
        &repo,
        &a,
        &[],
        "remaining staged branches have immutable descendants",
    );
}

#[test]
fn remote_trunk_is_not_pushed_and_repeated_landing_advances_local_bookmark() {
    let repo = Repo::new();
    let remote = repo.root().join("remote.git");
    let remote = remote.to_str().unwrap();
    success(
        &["git", "init", "--bare", remote],
        repo.command("git")
            .args(["init", "--bare", remote])
            .output()
            .unwrap(),
    );
    success(
        &["git", "push", remote, "trunk"],
        repo.command("git")
            .args(["push", remote, "trunk"])
            .output()
            .unwrap(),
    );
    repo.jj(&["git", "remote", "add", "origin", remote]);
    repo.jj(&["git", "fetch", "--remote", "origin"]);
    repo.jj(&["bookmark", "track", "trunk@origin"]);
    repo.jj(&[
        "config",
        "set",
        "--repo",
        "revset-aliases.\"trunk()\"",
        "trunk@origin",
    ]);
    let old_trunk = repo.id("trunk()");
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let c = repo.change("trunk", "C");
    repo.merge(&[&a, &b, &c], "megamerge");

    success(&["jj-land", "-r", &a], land(&repo, &a, &[]));
    success(&["jj-land", "-r", &b], land(&repo, &b, &[]));

    assert_eq!(repo.id("trunk"), b);
    assert_eq!(repo.id("trunk()"), old_trunk);
    assert_eq!(repo.parents(&c), set(&[&b]));
    assert!(repo.ids("merges()").is_empty());
    let remote_commit = success(
        &["git", "rev-parse"],
        repo.command("git")
            .args(["--git-dir", remote, "rev-parse", "refs/heads/trunk"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        repo.jj(&["log", "-r", "trunk@origin", "--no-graph", "-T", "commit_id"]),
        remote_commit.trim()
    );
}

#[test]
#[cfg(unix)]
fn land_alias_dispatches_through_installed_symlink() {
    let repo = Repo::new();
    std::fs::create_dir(repo.root().join("bin")).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_tools"), repo.root().join("bin/jj-land"))
        .unwrap();
    let config = repo.root().join("config.toml");
    let original = std::fs::read_to_string(&config).unwrap();
    let alias = r#"
[aliases]
land = ["util", "exec", "--", "jj-land", "-r"]
"#;
    std::fs::write(config, format!("{original}\n{alias}")).unwrap();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    repo.merge(&[&a, &b], "megamerge");

    repo.jj(&["land", &a]);

    assert_eq!(repo.id("trunk()"), a);
    assert_eq!(repo.parents(&b), set(&[&a]));
    assert!(repo.ids("merges()").is_empty());
}
