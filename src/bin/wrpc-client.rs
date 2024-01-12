
use bytes::Bytes;
use clap::Parser;




#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Handshake data
    #[arg(short, long, default_value = "")]
    data: Bytes,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let Args { data: _ } = Args::parse();

    todo!()
    //let (mut r, mut w) = io::split(conn);
    //let mut stdin = io::stdin();
    //let mut stdout = io::stdout();
    //try_join!(io::copy(&mut stdin, &mut w), io::copy(&mut r, &mut stdout))
    //    .context("connection failed")?;
    //Ok(())
}
