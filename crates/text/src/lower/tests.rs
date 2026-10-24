use std::collections::BTreeMap;

use super::{NEWER_CAPITALS, lower};
use crate::testing::cases;

fn mapping() -> BTreeMap<u32, Vec<u32>> {
    let rows: Vec<(u32, Vec<u32>)> = cases(env!("CARGO_MANIFEST_DIR"), "text", "lower");
    rows.into_iter().collect()
}

fn code_points(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

fn text_of(codes: &[u32]) -> String {
    let chars = codes.iter().map(|&code| char::from_u32(code));
    chars
        .collect::<Option<String>>()
        .unwrap_or_else(|| panic!("{codes:X?} holds a surrogate"))
}

#[test]
fn every_code_point_lowers_as_the_reference_does() {
    let mapping = mapping();
    let wrong: Vec<u32> = (0..=0x10_ffff)
        .filter_map(char::from_u32)
        .filter(|&c| {
            let code = u32::from(c);
            let want = mapping.get(&code).cloned().unwrap_or_else(|| vec![code]);
            code_points(&lower(&c.to_string())) != want
        })
        .map(u32::from)
        .collect();
    assert!(wrong.is_empty(), "differ at {wrong:X?}");
}

// The table holds exactly the characters where the standard library's newer mappings differ.
#[test]
fn the_table_holds_each_code_point_where_to_lowercase_differs() {
    let mapping = mapping();
    let differing: Vec<u32> = (0..=0x10_ffff)
        .filter_map(char::from_u32)
        .filter(|&c| {
            let code = u32::from(c);
            let want = mapping.get(&code).cloned().unwrap_or_else(|| vec![code]);
            code_points(&c.to_lowercase().to_string()) != want
        })
        .map(u32::from)
        .collect();
    let table: Vec<u32> = NEWER_CAPITALS.iter().cloned().flatten().collect();
    assert_eq!(differing, table);
    assert_eq!(table.len(), 55);
}

#[test]
fn a_final_sigma_reads_its_context_as_the_reference_does() {
    let rows: Vec<(Vec<u32>, Vec<u32>)> =
        cases(env!("CARGO_MANIFEST_DIR"), "text", "lower_strings");
    let wrong: Vec<String> = rows
        .iter()
        .filter(|(text, want)| code_points(&lower(&text_of(text))) != *want)
        .map(|(text, want)| format!("{text:X?} -> {want:X?}"))
        .collect();
    assert!(wrong.is_empty(), "differ: {wrong:?}");
}
