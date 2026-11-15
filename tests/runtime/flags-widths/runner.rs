use crate::client::test::flags_widths::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        roundtrip_flag17(tx, rx, &(Flag17::B0 | Flag17::B16), 7).await?,
        (Flag17::B0 | Flag17::B16, 7)
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        roundtrip_flag40(tx, rx, &(Flag40::B1 | Flag40::B39), 9).await?,
        (Flag40::B1 | Flag40::B39, 9)
    );
    Ok(())
}
