use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

pub fn read_orders(path: &Path) -> io::Result<Vec<Order>> {
    let reader = BufReader::new(File::open(path)?);
    Ok(reader.lines().map_while(Result::ok).filter_map(|line| parse_order(&line)).collect())
}

pub fn read_invoices(path: &Path) -> io::Result<Decimal> {
    let reader = BufReader::new(File::open(path)?);
    let mut total = Decimal::ZERO;
    for line in reader.lines() {
        let line = line?;
        if let Some(amount) = line.rsplit(';').next().and_then(Decimal::from_text) {
            total = total.saturating_add(amount);
        }
    }
    Ok(total)
}
