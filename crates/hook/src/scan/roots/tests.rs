use std::ffi::OsString;
use std::path::{Path, PathBuf};

use app::Services;

use super::{expanded, ignored, of_command};
use crate::testing::Sandbox;

// A sandbox with two repositories side by side, each with one commit, and a folder in no repository.
fn two_repos() -> (Sandbox, Services, PathBuf, PathBuf) {
    let sandbox = Sandbox::new();
    for name in ["one", "two"] {
        let root = sandbox.mkdir(name);
        sandbox.put(&format!("{name}/a.py"), "x = 1\n");
        drop(io::testing::run_git(
            &root,
            &["init", "--quiet", "-b", "main"],
            &io::testing::hermetic_env(&sandbox.path().join("home")),
            &identity(),
        ));
    }
    sandbox.mkdir("plain");
    let ceiling = sandbox.path().as_os_str().to_owned();
    let services = Services::new(&sandbox.env(&[("GIT_CEILING_DIRECTORIES", ceiling)]));
    let one = io::resolve(&sandbox.path().join("one")).expect("a resolved root");
    let two = io::resolve(&sandbox.path().join("two")).expect("a resolved root");
    (sandbox, services, one, two)
}

fn identity() -> io::testing::Identity {
    io::testing::Identity {
        name: "hook tests",
        email: "hook@example.invalid",
        date: "2026-02-01T00:00:00Z",
    }
}

#[test]
fn the_roots_are_those_of_the_folder_and_of_each_source_path_in_order() {
    let (sandbox, services, one, two) = two_repos();
    let cwd = sandbox.path().join("one");
    let command = format!(
        "cp a.py {}/b.py && cp a.py ../plain/c.py",
        sandbox.path().join("two").display()
    );

    let found = of_command(&services, None, &cwd, &command);

    assert_eq!(found.roots, [one, two]);
    assert_eq!(found.error, None);
}

#[test]
fn a_source_path_in_a_missing_folder_takes_the_nearest_folder_above() {
    let (sandbox, services, one, _) = two_repos();
    let cwd = sandbox.path().join("plain");
    let command = format!(
        "cat > {}/new/deep/x.py",
        sandbox.path().join("one").display()
    );

    let found = of_command(&services, None, &cwd, &command);

    assert_eq!(found.roots, [one]);
}

#[test]
fn a_folder_in_no_repository_gives_no_root_and_no_error() {
    let (sandbox, services, _, _) = two_repos();

    let found = of_command(
        &services,
        None,
        &sandbox.path().join("plain"),
        "echo x > y.txt",
    );

    assert_eq!(found.roots, Vec::<PathBuf>::new());
    assert_eq!(found.error, None);
}

#[test]
fn at_most_six_roots_are_kept() {
    let sandbox = Sandbox::new();
    let env = io::testing::hermetic_env(&sandbox.path().join("home"));
    let mut paths = Vec::new();
    for index in 0..8 {
        let name = format!("r{index}");
        let root = sandbox.mkdir(&name);
        drop(io::testing::run_git(
            &root,
            &["init", "--quiet", "-b", "main"],
            &env,
            &identity(),
        ));
        paths.push(format!("{}/{name}/x.py", sandbox.path().display()));
    }
    let command = format!("cp {}", paths.join(" "));
    let ceiling = sandbox.path().as_os_str().to_owned();
    let services = Services::new(&sandbox.env(&[("GIT_CEILING_DIRECTORIES", ceiling)]));
    sandbox.mkdir("plain");

    let found = of_command(&services, None, &sandbox.path().join("plain"), &command);

    assert_eq!(found.roots.len(), 6);
}

#[test]
fn a_leading_tilde_names_the_home_folder() {
    let home = Path::new("/home/user");
    assert_eq!(
        expanded(Some(home), Path::new("~/repo/a.py")),
        Path::new("/home/user/repo/a.py")
    );
    assert_eq!(
        expanded(Some(home), Path::new("~")),
        Path::new("/home/user")
    );
    assert_eq!(
        expanded(Some(home), Path::new("~other/a.py")),
        Path::new("~other/a.py")
    );
    assert_eq!(expanded(None, Path::new("~/a.py")), Path::new("~/a.py"));
}

#[test]
fn the_ignore_setting_lists_absolute_roots_with_the_tilde_expanded() {
    let sandbox = Sandbox::new();
    let kept = io::resolve(&sandbox.mkdir("kept")).expect("a resolved path");
    let under_home = io::resolve(&sandbox.path().join("under-home")).expect("a resolved path");
    let setting = OsString::from(format!(
        "{}::relative/path:~/under-home:/never/made",
        sandbox.path().join("kept").display()
    ));

    let roots = ignored(Some(&setting), Some(sandbox.path()));

    let expected: Vec<PathBuf> = vec![kept, under_home, "/never/made".into()];
    assert_eq!(roots, expected);
    assert_eq!(ignored(None, None), Vec::<PathBuf>::new());
}
