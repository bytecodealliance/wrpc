use crate::client::test::flags_widths::to_test::*;

pub async fn run(
    wrpc: &impl ::wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    assert_eq!(
        roundtrip_flag17(wrpc, (), &(Flag17::B0 | Flag17::B16), 7).await?,
        (Flag17::B0 | Flag17::B16, 7)
    );
    assert_eq!(
        roundtrip_flag40(wrpc, (), &(Flag40::B1 | Flag40::B39), 9).await?,
        (Flag40::B1 | Flag40::B39, 9)
    );
    Ok(())
}
