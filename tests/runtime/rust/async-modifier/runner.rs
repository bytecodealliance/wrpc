use my::inline::i::{f, one_argument, one_argument_and_result, one_result};

pub async fn run(
    clt: &impl wit_bindgen_wrpc::wrpc_transport::Invoke<Context = ()>,
) -> anyhow::Result<()> {
    f(clt, ()).await?;
    one_argument(clt, (), 1).await?;
    assert_eq!(one_result(clt, ()).await?, 2);
    assert_eq!(one_argument_and_result(clt, (), 3).await?, 4);
    Ok(())
}
