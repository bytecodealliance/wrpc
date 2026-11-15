use crate::client::test::list_in_variant::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::wit_bindgen_wrpc::anyhow::Result<()> {
    let hw: Vec<&str> = vec!["hello", "world"];
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_in_option(tx, rx, Some(hw.as_slice())).await?,
        "hello,world"
    );
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(list_in_option(tx, rx, None).await?, "none");

    let fbb = PayloadOrEmpty::WithData(vec!["foo".into(), "bar".into(), "baz".into()]);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(list_in_variant(tx, rx, &fbb).await?, "foo,bar,baz");
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(
        list_in_variant(tx, rx, &PayloadOrEmpty::Empty).await?,
        "empty"
    );

    let abc: Result<Vec<String>, String> = Ok(vec!["a".into(), "b".into(), "c".into()]);
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(list_in_result(tx, rx, &abc).await?, "a,b,c");
    let oops: Result<Vec<String>, String> = Err("oops".to_string());
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(list_in_result(tx, rx, &oops).await?, "err:oops");

    let hw2: Vec<&str> = vec!["hello", "world"];
    let (tx, rx) = wrpc.connect().await?;
    let s = list_in_option_with_return(tx, rx, Some(hw2.as_slice())).await?;
    assert_eq!(s.count, 2);
    assert_eq!(s.label, "hello,world");
    let (tx, rx) = wrpc.connect().await?;
    let s = list_in_option_with_return(tx, rx, None).await?;
    assert_eq!(s.count, 0);
    assert_eq!(s.label, "none");

    let xyz: Vec<&str> = vec!["x", "y", "z"];
    let (tx, rx) = wrpc.connect().await?;
    assert_eq!(top_level_list(tx, rx, &xyz).await?, "x,y,z");
    Ok(())
}
