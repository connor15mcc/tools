mod common;

use std::fs;

use common::{set, success, Repo};

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

#[test]
#[cfg(unix)]
fn stage_alias_inserts_pending_changes_into_existing_megamerge() {
    let repo = Repo::new();
    repo.install_stage_alias();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let merge = repo.merge(&[&a, &b], "megamerge");
    let c = repo.change(&merge, "C");
    repo.jj(&["new"]);
    let working_copy = repo.id("@");

    repo.jj(&["stage"]);

    assert_eq!(repo.parents(&merge), set(&[&a, &b, &c]));
    assert_eq!(repo.parents(&c), repo.ids("trunk()"));
    assert_eq!(repo.id("@"), working_copy);
    assert_eq!(repo.parents("@"), set(&[&merge]));
}

#[test]
#[cfg(unix)]
fn stage_alias_does_not_fall_back_when_active_stack_has_no_pending_work() {
    let repo = Repo::new();
    repo.install_stage_alias();
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    repo.change("trunk", "Unrelated");
    repo.merge(&[&a, &b], "megamerge");
    let before = repo.snapshot();

    let output = repo.command("jj").arg("stage").output().unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no commits match revset"));
    assert_eq!(repo.snapshot(), before);
}

#[test]
#[cfg(unix)]
fn stage_alias_creates_megamerge_from_siblings_without_including_empty_changes() {
    let repo = Repo::new();
    repo.install_stage_alias();
    let a = repo.change("trunk", "A");
    repo.jj(&["new", "trunk", "-m", "Empty plan"]);
    let empty_plan = repo.id("@");
    let b = repo.change("trunk", "B");

    repo.jj(&["stage"]);

    let merge = repo.id("merges()");
    repo.assert_megamerge_description(&merge);
    assert_eq!(repo.parents(&merge), set(&[&a, &b]));
    assert_eq!(repo.parents(&empty_plan), repo.ids("trunk()"));
    assert_eq!(repo.parents("@"), set(&[&merge]));
    assert_eq!(repo.ids("@ & empty()"), repo.ids("@"));
}

#[test]
#[cfg(unix)]
fn stage_alias_filters_immutable_foreign_and_non_descendant_changes() {
    let repo = Repo::new();
    repo.install_stage_alias();
    let immutable = repo.change("trunk", "Published");
    repo.jj(&[
        "config",
        "set",
        "--repo",
        "revset-aliases.\"immutable_heads()\"",
        &immutable,
    ]);
    success(
        &["jj", "new", "trunk", "-m", "Foreign"],
        repo.command("jj")
            .env("JJ_USER", "Someone Else")
            .env("JJ_EMAIL", "someone@example.invalid")
            .args(["new", "trunk", "-m", "Foreign"])
            .output()
            .unwrap(),
    );
    fs::write(repo.path().join("Foreign.txt"), "foreign\n").unwrap();
    let foreign = repo.id("@");
    let other_branch = repo.change("root()", "Other");
    let a = repo.change("trunk", "A");
    let b = repo.change("trunk", "B");
    let foreign_parents = repo.parents(&foreign);
    let other_parents = repo.parents(&other_branch);

    repo.jj(&["stage"]);

    let merge = repo.id("merges()");
    assert_eq!(repo.parents(&merge), set(&[&a, &b]));
    assert_eq!(repo.parents(&immutable), repo.ids("trunk()"));
    assert_eq!(repo.parents(&foreign), foreign_parents);
    assert_eq!(repo.parents(&other_branch), other_parents);
}

#[test]
#[cfg(unix)]
fn stage_alias_with_single_change_creates_no_merge() {
    let repo = Repo::new();
    repo.install_stage_alias();
    repo.change("trunk", "A");
    repo.jj(&["new"]);
    let before = repo.snapshot();

    repo.jj(&["stage"]);

    assert_eq!(repo.snapshot(), before);
    assert!(repo.ids("merges()").is_empty());
}
