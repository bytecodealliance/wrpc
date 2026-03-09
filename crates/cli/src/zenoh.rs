use serde_json::json;
use zenoh::{Config, Session};

/// Open a regular Zenoh session with configs supplied by an environment variable.
pub async fn connect() -> anyhow::Result<Session> {
    let cfg = if let Ok(cfg) = Config::from_env() { cfg } else {
        let mut config = Config::default();
        // Set mode
        config
            .insert_json5("mode", &json!("client").to_string())
            .unwrap();
        config
            .insert_json5(
                "connect/endpoints",
                &json!(["tcp/0.0.0.0:7447"]).to_string(),
            )
            .unwrap();

        config
    };

    let session = zenoh::open(cfg).await.unwrap();
    Ok(session)
}
