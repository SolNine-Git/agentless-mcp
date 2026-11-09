from pkg import fetch_rows

USERS = "select * from users"
AUDIT = "select * from audit"


def fetch_users(cursor):
    rows = fetch_rows(cursor, USERS)
    cursor.execute(AUDIT)
    seen = [column[0] for column in cursor.description]
    return rows, cursor.fetchall(), seen
