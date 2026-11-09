pub fn fetch_rows(cursor: &mut Cursor, query: &str) -> Vec<Row> {
    cursor.execute(query);
    let names: Vec<String> = cursor.description.iter().map(|column| column.name.clone()).collect();
    cursor.fetch_all().into_iter().map(|values| Row::zipped(&names, values)).collect()
}
