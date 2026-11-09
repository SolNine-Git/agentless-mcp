use crate::store::fetch_rows;

const USERS: &str = "select * from users";
const AUDIT: &str = "select * from audit";

pub fn fetch_users(cursor: &mut Cursor) -> (Vec<Row>, Vec<Row>) {
    let rows = fetch_rows(cursor, USERS);
    cursor.execute(AUDIT);
    let names: Vec<String> = cursor.description.iter().map(|column| column.name.clone()).collect();
    let audit = cursor.fetch_all().into_iter().map(|values| Row::zipped(&names, values)).collect();
    (rows, audit)
}
