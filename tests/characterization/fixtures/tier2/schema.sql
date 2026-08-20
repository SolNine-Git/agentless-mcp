CREATE TABLE teams (
    id INTEGER PRIMARY KEY
);

CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    team_id INTEGER REFERENCES teams(id)
);

CREATE VIEW active_users AS
SELECT id, team_id FROM users;
