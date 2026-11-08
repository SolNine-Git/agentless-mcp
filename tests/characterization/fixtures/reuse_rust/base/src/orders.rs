use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

pub fn read_orders(path: &Path) -> io::Result<Vec<Order>> {
    let reader = BufReader::new(File::open(path)?);
    Ok(reader.lines().map_while(Result::ok).filter_map(|line| parse_order(&line)).collect())
}
