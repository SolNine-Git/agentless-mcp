use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::{Repo, expand_user, resolve};
use crate::Error;
use crate::testing::{MANIFEST_DIR, Sandbox};

fn refusal(error: Error) -> kernel::Error {
    let Error::Kind(kind) = error else {
        panic!("a domain error, not {error:?}");
    };
    kind
}

#[test]
fn a_leading_tilde_becomes_the_home_directory() {
    let rows: Vec<(String, String, String)> =
        io::testing::cases(MANIFEST_DIR, "repo", "expand_user");
    for (path, home, expected) in rows {
        let found = expand_user(Path::new(&path), Some(OsStr::new(&home)));
        assert_eq!(
            found,
            PathBuf::from(&expected),
            "{path:?} with HOME={home:?}"
        );
    }
}

#[test]
fn a_tilde_that_names_a_user_stays_as_written() {
    let found = expand_user(Path::new("~root/x"), Some(OsStr::new("/home/u")));
    assert_eq!(found, PathBuf::from("~root/x"));
}

#[test]
fn any_directory_is_a_repository_root() {
    let sandbox = Sandbox::new();
    let plain = sandbox.mkdir("plain");
    let services = sandbox.services(sandbox.path());

    let repo = resolve(&services, Some(Path::new("plain"))).expect("a directory resolves");

    assert_eq!(repo.root, plain);
    assert_eq!(repo.config, io::ProjectConfig::default());
}

#[test]
fn a_file_is_not_a_repository() {
    let sandbox = Sandbox::new();
    let file = sandbox.put("file.txt", "text");
    let services = sandbox.services(sandbox.path());

    let refused = resolve(&services, Some(&file)).expect_err("a file is refused");

    let message = format!("not a directory: {}", file.display());
    assert_eq!(refusal(refused), kernel::Error::RepoResolution(message));
}

#[test]
fn without_a_root_the_enclosing_git_root_is_used() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("pkg/a.py", "x = 1\n")]);
    let services = sandbox.services(&root.join("pkg"));

    let repo = resolve(&services, None).expect("the git root resolves");

    assert_eq!(repo.root, root);
}

#[test]
fn without_a_root_or_a_git_root_the_start_directory_is_used() {
    let sandbox = Sandbox::new();
    let plain = sandbox.mkdir("plain");
    let services = sandbox.services(&plain);

    let repo = resolve(&services, None).expect("the start directory resolves");

    assert_eq!(repo.root, plain);
}

#[test]
fn the_project_config_is_read_with_the_root() {
    let sandbox = Sandbox::new();
    let root = sandbox.mkdir("repo");
    sandbox.put("repo/.agentless-mcp.json", r#"{"complexity_threshold": 7}"#);
    let services = sandbox.services(sandbox.path());

    let repo = Repo::load(&services, Path::new("repo")).expect("a directory loads");

    assert_eq!(repo.config.complexity_threshold, Some(7));
    assert_eq!(repo.config.path, Some(root.join(".agentless-mcp.json")));
}

#[test]
fn a_relative_root_with_an_unread_cwd_is_a_resolution_error() {
    let sandbox = Sandbox::new();
    let root = sandbox.mkdir("repo");
    let mut services = sandbox.services(sandbox.path());
    let unread = crate::EnvConfig::with_unread_cwd([], &std::io::Error::from_raw_os_error(2));
    services.cwd = Err(unread.cwd().expect_err("the cwd is unread").clone());
    let expected = kernel::Error::RepoResolution(
        "the start-up directory is unreadable: No such file or directory".to_owned(),
    );

    let relative = Repo::load(&services, Path::new("repo")).expect_err("no cwd to join");
    let from_cwd = resolve(&services, None).expect_err("no cwd to start from");
    let absolute = Repo::load(&services, &root).expect("an absolute root needs no cwd");

    assert_eq!(refusal(relative), expected);
    assert_eq!(refusal(from_cwd), expected);
    assert_eq!(absolute.root, root);
}
