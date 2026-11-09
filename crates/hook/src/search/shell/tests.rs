use text::CodePointString;

use super::single_file_read;

fn read(command: &str) -> Option<String> {
    let found = single_file_read(&CodePointString::from(command));
    found.map(|file| file.as_str().expect("a UTF-8 operand").to_owned())
}

#[test]
fn each_read_before_edit_view_of_one_file_names_that_file() {
    for (command, file) in [
        ("cat src/a.py", "src/a.py"),
        ("cat -n src/a.py", "src/a.py"),
        ("/usr/bin/cat src/a.py", "src/a.py"),
        ("cat \"my file.py\"", "my file.py"),
        ("nl -ba src/a.py", "src/a.py"),
        ("nl -w 3 src/a.py", "src/a.py"),
        ("bat src/a.py", "src/a.py"),
        ("batcat --line-range=1:20 src/a.py", "src/a.py"),
        ("bat -r 1:20 src/a.py", "src/a.py"),
        ("head -n 50 src/a.py", "src/a.py"),
        ("head -50 src/a.py", "src/a.py"),
        ("tail -n50 src/a.py", "src/a.py"),
        ("head --lines 20 src/a.py", "src/a.py"),
        ("sed -n '10,20p' src/a.py", "src/a.py"),
        ("sed -n 5p src/a.py", "src/a.py"),
        ("sed -n '10,$p' src/a.py", "src/a.py"),
        ("grep -n foo src/a.py", "src/a.py"),
        ("grep -n 'a|b' src/a.py", "src/a.py"),
        ("grep -n \"x > 1; y\" src/a.py", "src/a.py"),
        ("grep -e foo -n src/a.py", "src/a.py"),
        ("grep -A 3 foo src/a.py", "src/a.py"),
        ("grep -nA3 foo src/a.py", "src/a.py"),
        ("grep -- -x src/a.py", "src/a.py"),
        ("egrep 'a|b' src/a.py", "src/a.py"),
        ("fgrep foo src/a.py", "src/a.py"),
        ("rg -n 'fn main\\(' src/a.rs", "src/a.rs"),
        ("rg -t py foo src/a.py", "src/a.py"),
        ("rg --regexp=foo src/a.py", "src/a.py"),
    ] {
        assert_eq!(read(command).as_deref(), Some(file), "{command}");
    }
}

#[test]
fn a_command_that_is_more_than_one_view_of_one_file_names_no_file() {
    for command in [
        "",
        "cat",
        "cat a.py b.py",
        "cat a.py | head",
        "cat a.py|head",
        "cat a.py > out.txt",
        "cat a.py>out.txt",
        "cat a.py 2>/dev/null",
        "cat < a.py",
        "cat a.py; rm b.py",
        "cat a.py && ls",
        "cat a.py &",
        "cat `ls`",
        "cat a.py\nls",
        "cat 'unclosed",
        "LC_ALL=C cat a.py",
        "less a.py",
        "ls a.py",
        "sed -i 's/a/b/' a.py",
        "sed -n p a.py",
        "sed 1,5p a.py",
        "sed -n '1,5p' a.py b.py",
        "head -n 5",
        "grep foo",
        "grep foo a.py b.py",
        "grep -e foo",
        "rg foo",
    ] {
        assert_eq!(read(command), None, "{command:?}");
    }
}

// `cd DIR && VIEW` reads the view's file below DIR, as the shell would.
#[test]
fn one_cd_to_a_literal_folder_then_one_view_reads_the_file_below_it() {
    for (command, file) in [
        ("cd /r && cat src/a.py", "/r/src/a.py"),
        ("cd /r/ && cat a.py", "/r/a.py"),
        ("cd src && sed -n '1,5p' a.py", "src/a.py"),
        ("cd '/my dir' && head -n 5 a.py", "/my dir/a.py"),
        ("cd '/a&&b' && grep -n 'x|y' a.py", "/a&&b/a.py"),
        ("cd /r && cat /abs/a.py", "/abs/a.py"),
    ] {
        assert_eq!(read(command).as_deref(), Some(file), "{command}");
    }
}

#[test]
fn any_other_compound_with_a_cd_reads_no_file() {
    for command in [
        "cd /r && cat a.py | head",
        "cd /r && cat a.py && ls",
        "cd /r && cat a.py b.py",
        "cd /r && ls",
        "cd /r || cat a.py",
        "cd /r; cat a.py",
        "cd /r & cat a.py",
        "cd /r&&cat a.py",
        "cd && cat a.py",
        "cd -P /r && cat a.py",
        "cd - && cat a.py",
        "cd $HOME && cat a.py",
        "cd ~/r && cat a.py",
        "cd /r/* && cat a.py",
        "cd /r/{a,b} && cat a.py",
        "pushd /r && cat a.py",
        "ls /r && cat a.py",
        "cd /r && cd /s && cat a.py",
        "cd /r\n&& cat a.py",
    ] {
        assert_eq!(read(command), None, "{command:?}");
    }
}
