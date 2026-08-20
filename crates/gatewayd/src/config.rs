pub struct Config {
    bind_addr: String,
    _database_url: String,
}

impl Config {
    pub fn from_env() -> Self {
        Config {
            bind_addr: std::env::var("GATEWAY_BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:3000".into()),
            _database_url: std::env::var("DATABASE_URL").unwrap_or_default(),
        }
    }

    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }
}
