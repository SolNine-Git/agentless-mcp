use app::surface::Shape;
use prompts::messages::{ACCEPTS_SCHEMA, LISTED_OR};
use serde_json::json;

use super::{choice, listed, operation_owner, phrase, shown, unknown_tool};

#[test]
fn a_long_value_shows_its_head_and_its_tail_at_character_boundaries() {
    assert_eq!(shown(&json!([1, "x"])), "[1,\"x\"]");
    // 82 bytes: the cut keeps whole two-byte characters on both sides.
    let long = json!("é".repeat(40));
    let want = format!("\"{}...{}\"", "é".repeat(12), "é".repeat(11));
    assert_eq!(shown(&long), want);
}

#[test]
fn a_list_joins_its_last_name_with_the_template_word() {
    assert_eq!(listed(&[], &LISTED_OR), "");
    assert_eq!(listed(&["map"], &LISTED_OR), "map");
    assert_eq!(listed(&["map", "path"], &LISTED_OR), "map or path");
    assert_eq!(listed(&["a", "b", "c"], &LISTED_OR), "a, b or c");
}

// The surface has no unbounded integer and no other compound shape, so these two forms
// show only here.
#[test]
fn a_shape_the_surface_does_not_use_still_gets_a_form() {
    let open = Shape::Int {
        minimum: 1,
        maximum: None,
    };
    assert_eq!(phrase(&open), "an integer of 1 or more");
    assert_eq!(
        phrase(&Shape::AnyOf(&[Shape::Bool, Shape::Text])),
        ACCEPTS_SCHEMA
    );
}

#[test]
fn an_operation_name_points_to_the_tool_that_has_it() {
    let owner = operation_owner("find", "orient");
    assert_eq!(
        owner.as_deref(),
        Some("Call the tool symbols with operation find.")
    );
    assert_eq!(operation_owner("find", "symbols"), None);
    assert_eq!(
        operation_owner("communities", "orient").as_deref(),
        Some("Call the tool graph with operation communities.")
    );
    assert!(unknown_tool("overview").ends_with(" Call the tool symbols with operation overview."));
    assert!(!unknown_tool("repo_map").contains("Call the tool"));
}

// A choice of one is the name alone; a choice of more is one of a list.
#[test]
fn a_choice_of_one_name_is_the_name_alone() {
    assert_eq!(choice(&["map"]), "map");
    assert_eq!(choice(&["map", "path"]), "one of map or path");
    assert_eq!(phrase(&Shape::Literal(&["file"])), "file");
}
