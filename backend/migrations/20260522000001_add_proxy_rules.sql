CREATE TABLE IF NOT EXISTS proxy_rules (
    id SERIAL PRIMARY KEY,
    domain VARCHAR(255) UNIQUE NOT NULL,
    target_ip VARCHAR(255) NOT NULL,
    target_port INTEGER NOT NULL,
    container_id INTEGER REFERENCES containers(id),
    auth_enabled BOOLEAN DEFAULT false,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);
