use super::{source_paths, writes};

#[test]
fn each_write_form_is_a_write() {
    let forms = [
        "cat > src/a.py <<EOF",
        "echo x >> notes.txt",
        "echo x|tee out.rs",
        "sed -i 's/a/b/' src/a.py",
        "sed -n 1p x | sed --in-place s/a/b/ y",
        "perl -pi -e 's/a/b/' a.js",
        "python -c \"Path('a.py').write_text('x')\"",
        "python -c \"f = open('a.py', 'w')\"",
        "patch -p1 < fix.diff",
        "git apply fix.diff",
        "cp a.py b.py",
        "mv a.py b.py",
        "prettier --write src",
        "ruff format --in-place src",
        "dd if=a of=b",
    ];
    for command in forms {
        assert!(writes(command), "{command}");
    }
}

#[test]
fn reads_and_descriptor_redirects_are_no_write() {
    let forms = [
        "ls -la",
        "git status",
        "cat src/a.py",
        "cargo build 2>&1",
        "cargo test > /dev/null",
        "make >/dev/null 2>&1",
        "uv run script.py",
        "grep -n x src/a.py | head",
    ];
    for command in forms {
        assert!(!writes(command), "{command}");
    }
}

#[test]
fn a_redirect_with_a_target_after_blanks_is_a_write() {
    assert!(writes("echo x >   out.txt"));
    assert!(writes("echo x >out.txt"));
    assert!(!writes("echo x > "));
    assert!(!writes("echo x >"));
}

#[test]
fn only_tier_one_source_paths_are_named() {
    let command = "sed -i s/a/b/ src/a.py docs/x.md lib/b.rs ./c.ts notes.txt build/d.pyc";
    assert_eq!(source_paths(command), ["src/a.py", "lib/b.rs", "./c.ts"]);
    assert!(source_paths("ls -la").is_empty());
}
