//! Refuse a git revision range that contains an unsigned commit.
//!
//! `production` requires verified signatures on `main`. Squash-merge writes a
//! GitHub-signed commit there, which does not prove the pull-request head was
//! signed. Cloud Agents attach an HSM Ed25519 `gpgsig`; a `git commit` from a
//! session that turned `commit.gpgsign` off does not. This check reads the
//! commit object, not GitHub's verification API.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

/// True when the commit object carries a `gpgsig` or `gpgsig-sha256` header.
///
/// Only headers count. A commit whose *message* mentions `gpgsig` is unsigned.
pub(crate) fn commit_object_is_signed(object: &str) -> bool {
    let Some((headers, _)) = object.split_once("\n\n") else {
        return false;
    };
    headers
        .lines()
        .any(|line| line.starts_with("gpgsig ") || line.starts_with("gpgsig-sha256 "))
}

/// Commit ids in `base..head` whose objects carry no signature header.
pub(crate) fn unsigned_commits(repo: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    let range = format!("{base}..{head}");
    let listed = git_stdout(repo, &["rev-list", &range])?;
    let mut unsigned = Vec::new();
    for sha in listed.lines().filter(|line| !line.is_empty()) {
        let object = git_stdout(repo, &["cat-file", "-p", sha])?;
        if !commit_object_is_signed(&object) {
            unsigned.push(sha.to_string());
        }
    }
    Ok(unsigned)
}

/// Exit non-zero when `base..head` contains an unsigned commit.
pub(crate) fn check_range(repo: &Path, base: &str, head: &str) -> Result<()> {
    let unsigned = unsigned_commits(repo, base, head)?;
    if unsigned.is_empty() {
        eprintln!("navigator: every commit in {base}..{head} is signed");
        return Ok(());
    }
    bail!(
        "{} unsigned commit(s) in {base}..{head}:\n{}",
        unsigned.len(),
        unsigned.join("\n")
    )
}

fn git_stdout(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("git stdout is UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    const UNSIGNED: &str = "\
tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904
author Dev <dev@example.com> 1 +0000
committer Dev <dev@example.com> 1 +0000

unsigned
";

    const SSH_SIGNED: &str = "\
tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904
author Dev <dev@example.com> 1 +0000
committer Dev <dev@example.com> 1 +0000
gpgsig -----BEGIN SSH SIGNATURE-----
 U1NIU0lHAAAAAQ==
 -----END SSH SIGNATURE-----

signed
";

    const PGP_SIGNED: &str = "\
tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904
author Dev <dev@example.com> 1 +0000
committer Dev <dev@example.com> 1 +0000
gpgsig -----BEGIN PGP SIGNATURE-----
 
 iQIzBAAB
 =abcd
 -----END PGP SIGNATURE-----

signed
";

    const SHA256_SIGNED: &str = "\
tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904
author Dev <dev@example.com> 1 +0000
committer Dev <dev@example.com> 1 +0000
gpgsig-sha256 -----BEGIN SSH SIGNATURE-----
 U1NIU0lHAAAAAQ==
 -----END SSH SIGNATURE-----

signed
";

    const SIGNATURE_ONLY_IN_MESSAGE: &str = "\
tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904
author Dev <dev@example.com> 1 +0000
committer Dev <dev@example.com> 1 +0000

gpgsig -----BEGIN SSH SIGNATURE-----
 U1NIU0lHAAAAAQ==
 -----END SSH SIGNATURE-----
";

    #[test]
    fn an_unsigned_commit_object_is_unsigned() {
        assert!(!commit_object_is_signed(UNSIGNED));
    }

    #[test]
    fn an_ssh_gpgsig_header_is_a_signature() {
        assert!(commit_object_is_signed(SSH_SIGNED));
    }

    #[test]
    fn a_pgp_gpgsig_header_is_a_signature() {
        assert!(commit_object_is_signed(PGP_SIGNED));
    }

    #[test]
    fn a_sha256_signature_header_is_a_signature() {
        assert!(commit_object_is_signed(SHA256_SIGNED));
    }

    #[test]
    fn a_gpgsig_in_the_commit_message_is_not_a_signature() {
        assert!(!commit_object_is_signed(SIGNATURE_ONLY_IN_MESSAGE));
    }

    #[test]
    fn a_range_of_unsigned_commits_is_refused() {
        let repo = init_repo();
        let base = commit_file(repo.path(), "base.txt", "base\n", "base");
        let head = commit_file(repo.path(), "head.txt", "head\n", "head");
        let unsigned = unsigned_commits(repo.path(), &base, &head).expect("rev-list");
        assert_eq!(unsigned, vec![head.clone()]);
        let error = check_range(repo.path(), &base, &head).expect_err("unsigned range");
        assert!(error.to_string().contains(&head), "{error}");
    }

    #[test]
    fn a_range_of_signed_commit_objects_is_accepted() {
        let repo = init_repo();
        let base = commit_file(repo.path(), "base.txt", "base\n", "base");
        let tree = git(repo.path(), &["rev-parse", "HEAD^{tree}"]);
        let head = hash_commit(repo.path(), &signed_commit_object(&tree, &base));
        git(repo.path(), &["update-ref", "refs/heads/signed", &head]);
        check_range(repo.path(), &base, &head).expect("signed range");
        assert!(unsigned_commits(repo.path(), &base, &head)
            .expect("rev-list")
            .is_empty());
    }

    fn init_repo() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        git(dir.path(), &["init", "--initial-branch=main"]);
        dir
    }

    fn commit_file(repo: &Path, name: &str, contents: &str, message: &str) -> String {
        fs::write(repo.join(name), contents).expect("write");
        git(repo, &["add", name]);
        git(
            repo,
            &[
                "-c",
                "user.name=Dev",
                "-c",
                "user.email=dev@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                message,
            ],
        );
        git(repo, &["rev-parse", "HEAD"])
    }

    fn signed_commit_object(tree: &str, parent: &str) -> String {
        format!(
            "\
tree {tree}
parent {parent}
author Dev <dev@example.com> 1 +0000
committer Dev <dev@example.com> 1 +0000
gpgsig -----BEGIN SSH SIGNATURE-----
 U1NIU0lHAAAAAQ==
 -----END SSH SIGNATURE-----

signed
"
        )
    }

    fn hash_commit(repo: &Path, object: &str) -> String {
        let mut child = Command::new("git")
            .args(["-C"])
            .arg(repo)
            .args(["hash-object", "-t", "commit", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("hash-object");
        std::io::Write::write_all(&mut child.stdin.take().expect("stdin"), object.as_bytes())
            .expect("write commit");
        let output = child.wait_with_output().expect("wait hash-object");
        assert!(output.status.success(), "{:?}", output.stderr);
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    }

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(["-C"])
            .arg(repo)
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    }
}
