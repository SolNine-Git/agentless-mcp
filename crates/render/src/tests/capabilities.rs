use super::rows::{CapabilityRow, ReceiptRow, cases};
use crate::capabilities::render_capabilities;
use crate::capabilities_text;
use crate::envelope::wrap_json;

#[test]
fn each_report_renders_as_the_fixture_says() {
    let all: Vec<(CapabilityRow, String)> = cases("capabilities", "text");
    for (row, expected) in all {
        let config = row.config();
        assert_eq!(render_capabilities(&row.capabilities(&config)), expected);
    }
}

#[test]
fn each_document_wraps_as_the_fixture_says() {
    let all: Vec<(ReceiptRow, CapabilityRow, usize, String)> = cases("capabilities", "json");
    for (receipt_row, row, ceiling, expected) in all {
        let (receipt_config, config) = (receipt_row.config(), row.config());
        let receipt = receipt_row.receipt(&receipt_config);
        let wrapped = wrap_json(&receipt, &row.capabilities(&config), ceiling);
        let wrapped = wrapped.unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(wrapped, expected, "at {ceiling}");
    }
}

#[test]
fn the_text_form_wraps_the_report_under_its_receipt() {
    let all: Vec<(ReceiptRow, CapabilityRow, String)> = cases("capabilities", "wrapped");
    for (receipt_row, row, expected) in all {
        let (receipt_config, config) = (receipt_row.config(), row.config());
        let receipt = receipt_row.receipt(&receipt_config);
        assert_eq!(
            capabilities_text(&receipt, &row.capabilities(&config)),
            expected
        );
    }
}
