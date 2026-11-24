use crate::client::test::list_in_variant::to_test::*;

pub async fn run(
    wrpc: &crate::Client,
) -> ::anyhow::Result<()> {
    let hw: Vec<&str> = vec!["hello", "world"];
    let mut clt = wrpc.connect().await?;
    assert_eq!(
        list_in_option(&mut clt, Some(hw.as_slice())).await?,
        "hello,world"
    );
    assert_eq!(list_in_option(&mut clt, None).await?, "none");

    let fbb = PayloadOrEmpty::WithData(vec!["foo".into(), "bar".into(), "baz".into()]);
    assert_eq!(list_in_variant(&mut clt, &fbb).await?, "foo,bar,baz");
    assert_eq!(
        list_in_variant(&mut clt, &PayloadOrEmpty::Empty).await?,
        "empty"
    );

    let abc: Result<Vec<String>, String> = Ok(vec!["a".into(), "b".into(), "c".into()]);
    assert_eq!(list_in_result(&mut clt, &abc).await?, "a,b,c");
    let oops: Result<Vec<String>, String> = Err("oops".to_string());
    assert_eq!(list_in_result(&mut clt, &oops).await?, "err:oops");

    let hw2: Vec<&str> = vec!["hello", "world"];
    let s = list_in_option_with_return(&mut clt, Some(hw2.as_slice())).await?;
    assert_eq!(s.count, 2);
    assert_eq!(s.label, "hello,world");
    let s = list_in_option_with_return(&mut clt, None).await?;
    assert_eq!(s.count, 0);
    assert_eq!(s.label, "none");

    let xyz: Vec<&str> = vec!["x", "y", "z"];
    assert_eq!(top_level_list(&mut clt, &xyz).await?, "x,y,z");
    Ok(())
}
