use std::ffi::{OsStr, OsString};
use std::fs;

use io::testing::Tree;

use super::{EnvConfig, Services};

// A git that only writes its environment where ENV_LOG says.
const ENV_GIT: &str = "#!/bin/sh\nenv > \"$ENV_LOG\"\n";

#[test]
fn the_first_entry_of_a_repeated_name_wins() {
    let tree = Tree::new().expect("a temporary tree");
    tree.create_dir("bin").expect("a bin directory");
    tree.write_file("bin/git", ENV_GIT).expect("a fake git");
    tree.set_mode("bin/git", 0o755).expect("an executable git");
    let (first, second) = (tree.join("h1"), tree.join("h2"));
    let log = tree.join("env.log");
    let path = std::env::var_os("PATH").expect("the fake git finds env through PATH");
    let mut searched = tree.join("bin").into_os_string();
    searched.push(":");
    searched.push(path);
    let vars = [
        ("PATH", searched),
        ("HOME", first.clone().into_os_string()),
        ("GIT_CONFIG_GLOBAL", OsString::from("/dev/null")),
        ("PATH", OsString::from("/nonexistent")),
        ("HOME", second.into_os_string()),
        ("GIT_CONFIG_GLOBAL", tree.join("config").into_os_string()),
        ("ENV_LOG", log.clone().into_os_string()),
    ];
    let env = EnvConfig::new(
        vars.map(|(name, value)| (OsString::from(name), value)),
        tree.path().to_path_buf(),
    );

    let services = Services::new(&env);
    drop(services.git.root(tree.path()));

    assert_eq!(env.var("HOME"), Some(first.as_os_str()));
    assert_eq!(env.var("GIT_CONFIG_GLOBAL"), Some(OsStr::new("/dev/null")));
    assert_eq!(env.vars().len(), 4);
    assert_eq!(services.home.as_deref(), Some(first.as_os_str()));
    let cache_root = services.cache_root.expect("a cache root");
    assert!(cache_root.path.starts_with(&first), "{cache_root:?}");
    let child = fs::read_to_string(&log).expect("the fake git ran from the first PATH");
    let homes: Vec<&str> = child
        .lines()
        .filter(|line| line.starts_with("HOME="))
        .collect();
    assert_eq!(homes, [format!("HOME={}", first.display())]);
    let configs: Vec<&str> = child
        .lines()
        .filter(|line| line.starts_with("GIT_CONFIG_GLOBAL="))
        .collect();
    assert_eq!(configs, ["GIT_CONFIG_GLOBAL=/dev/null"]);
}

#[test]
fn an_unread_start_directory_keeps_the_first_entry_too() {
    let vars = [("HOME", "/first"), ("HOME", "/second")]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
    let error = std::io::Error::from_raw_os_error(2);

    let env = EnvConfig::with_unread_cwd(vars, &error);

    assert_eq!(env.var("HOME"), Some(OsStr::new("/first")));
    assert_eq!(env.vars().len(), 1);
}
