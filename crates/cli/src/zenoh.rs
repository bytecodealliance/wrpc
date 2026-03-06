use zenoh::{Config, Session};

/// Open a regular Zenoh session with configs supplied by an environment variable.
pub async fn connect() -> anyhow::Result<Session> {
    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(_) => Config::default(),
    };

    let session = zenoh::open(cfg)
        .await.unwrap();
    Ok(session)
}