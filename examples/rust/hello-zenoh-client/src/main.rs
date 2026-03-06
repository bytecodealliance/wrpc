use std::sync::Arc;
use anyhow::Context as _;
use zenoh::{Config};
use std::time::Instant;
use serde_json::json;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc-examples:hello/handler": generate
        }
    });
}


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(_) => {
            let mut config = Config::default();
            // Set mode
            config.insert_json5("mode", &json!("peer").to_string()).unwrap();
            config.insert_json5(
                "connect/endpoints",
                &json!(["tcp/0.0.0.0:7447"]).to_string(),
            ).unwrap();

            config
        },
    };

    let session = zenoh::open(cfg)
                            .await
                            .expect("Failed to open a Zenoh session");

    let arc_session = Arc::new(session);

    let prefix = Arc::<str>::from("");

    let wrpc = wrpc_transport_zenoh::Client::new(arc_session, prefix)
                    .await
                    .context("failed to construct transport client")?;

    // From hello-nats-client
    // let hello = bindings::wrpc_examples::hello::handler::hello(&wrpc, ())
    //     .await
    //     .context("failed to invoke `wrpc-examples.hello/handler.hello`")?;
    
    // eprintln!("NOT_USED_var:prefix: {hello}");

    // actually useful stuff
    let ipv4_address = bindings::wrpc_examples::hello::handler::Ipv4Address{octets: (0,0,0,0)};
    let timespec = bindings::wrpc_examples::hello::handler::Timespec{tv_sec:-111, tv_nsec:111};
    let tr = bindings::wrpc_examples::hello::handler::Tracking {
        ref_id: 32,
        ip_addr: ipv4_address,
        stratum: 16,
        leap_status: 16,
        ref_time: timespec,
        current_correction: 64.0,
        last_offset: 64.0,
        rms_offset: 64.0,
        freq_ppm: 64.0,
        resid_freq_ppm: 64.0,
        skew_ppm: 64.0,
        root_delay: 64.0,
        root_dispersion: 64.0,
        last_update_interval: 64.0,
    };

    let start = Instant::now();
    let limit = 100001;
    for i in 1..limit {
        let t1 = bindings::wrpc_examples::hello::handler::get_tracking(&wrpc, (), &tr)
        .await
        .context("failed to invoke `wrpc-examples.hello/handler.hello`")?;
        
        // tiny sleep to allow buffers to clear
        // tokio::time::sleep(tokio::time::Duration::from_micros(33)).await;

        // println!("Call {i} {:?}", t1.ref_id);
    }

    // Record the time after the loop finishes and calculate the duration
    let duration = start.elapsed();

    // Print the elapsed time
    // The {:?} formatter uses the Debug implementation for Duration
    println!("Time elapsed in the loop is for {limit} calls. Time Elapsed: {duration:?}");

    Ok(())
}
