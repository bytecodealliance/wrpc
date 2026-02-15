use zenoh::{Config, Session};

/// Open a regular Zenoh session with configs supplied by an environment variable.
pub async fn connect() -> anyhow::Result<Session> {
    let cfg = Config::from_env().expect("Missing environment variable 'ZENOH_CONFIG'");

    let session = zenoh::open(cfg)
        .await.unwrap();
    Ok(session)
}