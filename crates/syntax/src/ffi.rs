use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_bash() -> *const ();
    fn tree_sitter_c() -> *const ();
    fn tree_sitter_cpp() -> *const ();
    fn tree_sitter_c_sharp() -> *const ();
    fn tree_sitter_go() -> *const ();
    fn tree_sitter_hcl() -> *const ();
    fn tree_sitter_java() -> *const ();
    fn tree_sitter_javascript() -> *const ();
    fn tree_sitter_json() -> *const ();
    fn tree_sitter_kotlin() -> *const ();
    fn tree_sitter_lua() -> *const ();
    fn tree_sitter_php() -> *const ();
    fn tree_sitter_python() -> *const ();
    fn tree_sitter_ruby() -> *const ();
    fn tree_sitter_rust() -> *const ();
    fn tree_sitter_scala() -> *const ();
    fn tree_sitter_sql() -> *const ();
    fn tree_sitter_swift() -> *const ();
    fn tree_sitter_toml() -> *const ();
    fn tree_sitter_tsx() -> *const ();
    fn tree_sitter_typescript() -> *const ();
    fn tree_sitter_yaml() -> *const ();
}

// SAFETY: build.rs compiles and links each function from the vendored grammar. Each takes no
// arguments and returns a pointer to a static TSLanguage, as LanguageFn::from_raw requires.
pub(crate) const LANGUAGE_FNS: [LanguageFn; 22] = unsafe {
    [
        LanguageFn::from_raw(tree_sitter_bash),
        LanguageFn::from_raw(tree_sitter_c),
        LanguageFn::from_raw(tree_sitter_cpp),
        LanguageFn::from_raw(tree_sitter_c_sharp),
        LanguageFn::from_raw(tree_sitter_go),
        LanguageFn::from_raw(tree_sitter_hcl),
        LanguageFn::from_raw(tree_sitter_java),
        LanguageFn::from_raw(tree_sitter_javascript),
        LanguageFn::from_raw(tree_sitter_json),
        LanguageFn::from_raw(tree_sitter_kotlin),
        LanguageFn::from_raw(tree_sitter_lua),
        LanguageFn::from_raw(tree_sitter_php),
        LanguageFn::from_raw(tree_sitter_python),
        LanguageFn::from_raw(tree_sitter_ruby),
        LanguageFn::from_raw(tree_sitter_rust),
        LanguageFn::from_raw(tree_sitter_scala),
        LanguageFn::from_raw(tree_sitter_sql),
        LanguageFn::from_raw(tree_sitter_swift),
        LanguageFn::from_raw(tree_sitter_toml),
        LanguageFn::from_raw(tree_sitter_tsx),
        LanguageFn::from_raw(tree_sitter_typescript),
        LanguageFn::from_raw(tree_sitter_yaml),
    ]
};
