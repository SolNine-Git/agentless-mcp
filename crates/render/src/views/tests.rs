use app::FileView;

use super::overview_block;

fn view(path: &str, language: &str, text: &str) -> FileView {
    FileView {
        path: path.to_owned(),
        language: language.to_owned(),
        text: text.to_owned(),
        error: String::new(),
        refused: false,
    }
}

#[test]
fn a_data_file_names_its_nested_keys_as_parent_child_and_code_as_class_method() {
    let data = overview_block(&view("package.json", "json", "name = \"x\"  @2\n"));
    let code = overview_block(&view("a.py", "python", "def f(): ...\n"));
    assert_eq!(
        data,
        "package.json  (json)\n  stable ids: json:package.json::<QualifiedName> -- nested keys \
         qualify as parent.child\nname = \"x\"  @2\n"
    );
    assert_eq!(
        code,
        "a.py  (python)\n  stable ids: py:a.py::<QualifiedName> -- nested symbols qualify as \
         Class.method\ndef f(): ...\n"
    );
}
