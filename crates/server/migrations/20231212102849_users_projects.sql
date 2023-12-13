CREATE TABLE users (
    id SERIAL,
    email VARCHAR(255) UNIQUE NOT NULL,
    name VARCHAR(255) DEFAULT NULL,
    PRIMARY KEY (id)
);

CREATE TABLE users_projects (
    user_id INTEGER NOT NULL,
    project_id INTEGER NOT NULL,
    PRIMARY KEY (user_id, project_id),
    FOREIGN KEY (user_id) REFERENCES users (id),
    FOREIGN KEY (project_id) REFERENCES projects (id)
);