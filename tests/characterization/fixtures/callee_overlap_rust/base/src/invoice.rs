use crate::helpers::{inv_footer, inv_header, inv_lines, inv_tax};


pub fn render_invoice(total: i64) -> i64 {
    let head = inv_header(total);
    let body = inv_lines(head);
    let taxed = inv_tax(body);
    inv_footer(taxed)
}
